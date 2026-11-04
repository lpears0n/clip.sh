//! Tone mapping HDR game frames into SDR clips.
//!
//! An HDR window reaches us as scRGB: linear light with BT.709 primaries,
//! where 1.0 is 80 nits and highlights go well past that. Clips are SDR, so
//! each pixel is scaled so the display's SDR white becomes 1.0, the part of
//! the range above a knee is rolled off smoothly so the display's peak lands
//! on 1.0 (rather than everything above SDR white clipping to flat white),
//! and the result is encoded with the sRGB curve.
//!
//! The roll-off works on the brightest channel and scales all three by the
//! same factor, so colours keep their hue as they brighten. Below the knee
//! nothing changes: what was SDR-bright stays as it was.
//!
//! [`SHADER`] does this on the GPU (Windows); [`map`] is the same maths on
//! the CPU, for tests and as its reference.

use crate::HdrDisplay;

/// scRGB's reference white: 1.0 is this many nits.
const SCRGB_NITS: f32 = 80.0;
/// Below this fraction of SDR white, values pass through unchanged.
pub const KNEE: f32 = 0.75;
/// Assumed peak for a display that doesn't report one.
const DEFAULT_PEAK_NITS: f32 = 1000.0;

/// The constants the curve needs for one display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// scRGB value × `scale` = fraction of SDR white.
    pub scale: f32,
    /// The display's peak as a multiple of SDR white (≥ 1).
    pub peak: f32,
    pub knee: f32,
}

impl Params {
    pub fn for_display(d: &HdrDisplay) -> Self {
        let white = if d.sdr_white_nits > 0.0 {
            d.sdr_white_nits
        } else {
            SCRGB_NITS
        };
        let peak = if d.peak_nits > 0.0 {
            d.peak_nits
        } else {
            DEFAULT_PEAK_NITS
        };
        Self {
            scale: SCRGB_NITS / white,
            peak: (peak / white).max(1.0),
            knee: KNEE,
        }
    }
}

/// The roll-off on one value, in multiples of SDR white: unchanged up to
/// the knee, then an extended-Reinhard curve on the rest that meets the knee
/// with the same slope and reaches exactly 1.0 at the display's peak.
pub fn curve(x: f32, p: &Params) -> f32 {
    if p.peak <= 1.0 || x <= p.knee {
        return x.min(1.0);
    }
    let range = 1.0 - p.knee;
    let e = (x - p.knee) / range;
    let ew = (p.peak - p.knee) / range;
    let y = p.knee + range * e * (1.0 + e / (ew * ew)) / (1.0 + e);
    y.min(1.0)
}

/// One scRGB pixel to sRGB-encoded SDR, each channel in 0..=1.
pub fn map(rgb: [f32; 3], p: &Params) -> [f32; 3] {
    let c = rgb.map(|v| (v * p.scale).max(0.0));
    let m = c[0].max(c[1]).max(c[2]);
    let k = if m > p.knee { curve(m, p) / m } else { 1.0 };
    c.map(|v| srgb((v * k).clamp(0.0, 1.0)))
}

/// The sRGB transfer function (linear 0..=1 to encoded).
pub fn srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Direct3D 11 shaders (shader model 4) for one full-frame pass: a
/// triangle covering the viewport, sampling the captured FP16 window from
/// `uv_offset` over `uv_scale` (its client area) and writing tone-mapped
/// sRGB. The constant buffer is [`ShaderParams`].
pub const SHADER: &str = r#"
cbuffer Params : register(b0)
{
    float2 uv_offset;
    float2 uv_scale;
    float scale;
    float peak;
    float knee;
    float pad;
};

Texture2D<float4> frame : register(t0);
SamplerState linear_clamp : register(s0);

struct VsOut
{
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
};

VsOut vs(uint id : SV_VertexID)
{
    VsOut o;
    float2 t = float2((id << 1) & 2, id & 2);
    o.pos = float4(t * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    o.uv = uv_offset + t * uv_scale;
    return o;
}

float curve(float x)
{
    if (peak <= 1.0 || x <= knee)
        return min(x, 1.0);
    float range = 1.0 - knee;
    float e = (x - knee) / range;
    float ew = (peak - knee) / range;
    float y = knee + range * e * (1.0 + e / (ew * ew)) / (1.0 + e);
    return min(y, 1.0);
}

float3 srgb(float3 c)
{
    float3 lo = c * 12.92;
    float3 hi = 1.055 * pow(max(c, 0.0031308), 1.0 / 2.4) - 0.055;
    return c <= 0.0031308 ? lo : hi;
}

float4 ps(VsOut i) : SV_Target
{
    float3 c = max(frame.Sample(linear_clamp, i.uv).rgb * scale, 0.0);
    float m = max(c.r, max(c.g, c.b));
    if (m > knee)
        c *= curve(m) / m;
    return float4(srgb(saturate(c)), 1.0);
}
"#;

/// The shader's constant buffer, laid out as HLSL packs it (32 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShaderParams {
    pub uv_offset: [f32; 2],
    pub uv_scale: [f32; 2],
    pub scale: f32,
    pub peak: f32,
    pub knee: f32,
    pub pad: f32,
}

impl ShaderParams {
    /// `crop`: the part of the captured texture to show, as
    /// (x, y, width, height) over the texture's (width, height).
    pub fn new(p: &Params, crop: (f32, f32, f32, f32), texture: (f32, f32)) -> Self {
        let (x, y, w, h) = crop;
        let (tw, th) = (texture.0.max(1.0), texture.1.max(1.0));
        Self {
            uv_offset: [x / tw, y / th],
            uv_scale: [w / tw, h / th],
            scale: p.scale,
            peak: p.peak,
            knee: p.knee,
            pad: 0.0,
        }
    }
}

/// Where a `src`-sized picture goes inside a `dst`-sized frame: as large
/// as fits with its shape kept, centred (letterboxed or pillarboxed).
/// Returns (x, y, width, height).
pub fn fit(src: (f32, f32), dst: (f32, f32)) -> (f32, f32, f32, f32) {
    let (sw, sh) = (src.0.max(1.0), src.1.max(1.0));
    let k = (dst.0 / sw).min(dst.1 / sh);
    let (w, h) = (sw * k, sh * k);
    ((dst.0 - w) / 2.0, (dst.1 - h) / 2.0, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(white: f32, peak: f32) -> Params {
        Params::for_display(&HdrDisplay {
            sdr_white_nits: white,
            peak_nits: peak,
        })
    }

    #[test]
    fn sdr_range_below_the_knee_is_untouched() {
        let p = display(200.0, 1000.0);
        // Half of SDR white (100 nits) in scRGB is 1.25.
        let out = map([1.25, 1.25, 1.25], &p);
        assert!((out[0] - srgb(0.5)).abs() < 1e-6, "{out:?}");
        assert_eq!(map([0.0; 3], &p), [0.0; 3]);
    }

    #[test]
    fn highlights_roll_off_to_the_peak() {
        let p = display(200.0, 1000.0);
        // SDR white sits just under the top, leaving room above it.
        let white = curve(1.0, &p);
        assert!((0.85..0.95).contains(&white), "{white}");
        // The display's peak reaches 1.0, and nothing goes past it.
        assert!((curve(p.peak, &p) - 1.0).abs() < 1e-6);
        assert_eq!(curve(p.peak * 3.0, &p), 1.0);
    }

    #[test]
    fn the_curve_is_smooth_and_rising() {
        let p = display(240.0, 800.0);
        let eps = 1e-4;
        // Same value and slope either side of the knee.
        let below = curve(p.knee - eps, &p);
        let above = curve(p.knee + eps, &p);
        assert!((above - below - 2.0 * eps).abs() < 1e-5, "{below} {above}");
        let mut last = 0.0;
        for i in 1..=2000 {
            let y = curve(i as f32 / 200.0, &p);
            assert!(y >= last, "falls at {}", i as f32 / 200.0);
            last = y;
        }
    }

    #[test]
    fn hue_is_kept_as_it_brightens() {
        let p = display(200.0, 1000.0);
        // A bright orange highlight: the channels keep their proportions
        // (in linear light) instead of each clipping to 1.0.
        let lin = |c: f32| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let out = map([8.0, 4.0, 0.0], &p).map(lin);
        assert!((out[1] / out[0] - 0.5).abs() < 1e-3, "{out:?}");
        assert_eq!(out[2], 0.0);
    }

    #[test]
    fn displays_without_headroom_just_clip() {
        let p = display(400.0, 400.0);
        assert_eq!(curve(0.9, &p), 0.9);
        assert_eq!(curve(1.5, &p), 1.0);
        // An unreported peak assumes 1000 nits; an unreported white, 80.
        let p = display(0.0, 0.0);
        assert_eq!(p.scale, 1.0);
        assert!((p.peak - 12.5).abs() < 1e-6);
    }

    #[test]
    fn shader_constants_match_the_hlsl_layout() {
        assert_eq!(std::mem::size_of::<ShaderParams>(), 32);
        let s = ShaderParams::new(
            &display(200.0, 1000.0),
            (8.0, 31.0, 1920.0, 1080.0),
            (1936.0, 1119.0),
        );
        assert!((s.uv_offset[1] - 31.0 / 1119.0).abs() < 1e-7);
        assert!((s.uv_scale[0] - 1920.0 / 1936.0).abs() < 1e-7);
        for name in ["uv_offset", "uv_scale", "scale", "peak", "knee"] {
            assert!(SHADER.contains(name), "{name}");
        }
    }

    #[test]
    fn fit_letterboxes_into_the_frame() {
        // Ultrawide into 16:9: bars top and bottom.
        let (x, y, w, h) = fit((3440.0, 1440.0), (1920.0, 1080.0));
        assert!(x.abs() < 1e-3 && (w - 1920.0).abs() < 1e-3, "{x} {w}");
        assert!(
            (h - 803.72).abs() < 0.01 && (y - 138.14).abs() < 0.01,
            "{y} {h}"
        );
        // Same shape: fills it.
        assert_eq!(
            fit((1280.0, 720.0), (1920.0, 1080.0)),
            (0.0, 0.0, 1920.0, 1080.0)
        );
    }
}
