//! Render an edited clip: trim to a range and mix the audio tracks at chosen
//! levels into one track (so the result plays the same everywhere).

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, bail};
use gstreamer as gst;
use gstreamer::prelude::*;

use crate::{
    capture::VideoConfig,
    encoder::{Codec, Encoder},
    media,
};

#[derive(Debug, Clone)]
pub struct Edit {
    pub start: gst::ClockTime,
    pub end: gst::ClockTime,
    /// Linear gain per source audio track, in file order (1.0 = unchanged).
    /// Missing entries default to 1.0.
    pub volumes: Vec<f64>,
}

const AAC_BITRATE: i32 = 192_000;

/// Re-encode `input[start..end]` to `output` as H.264 (edits are for sharing,
/// so they favour compatibility). `encoder` must support H.264. Blocks until
/// done.
pub fn render(
    input: &Path,
    output: &Path,
    edit: &Edit,
    encoder: Encoder,
    cfg: &VideoConfig,
) -> Result<()> {
    let cfg = &VideoConfig {
        codec: Codec::H264,
        ..cfg.clone()
    };
    if edit.end <= edit.start {
        bail!("end must be after start");
    }
    let audio_tracks = media::inspect(input)?.audio_tracks;
    if let Some(dir) = output.parent() {
        std::fs::create_dir_all(dir)?;
    }

    let pipeline = gst::Pipeline::new();
    let dec = gst::ElementFactory::make("uridecodebin")
        .property("uri", gst::glib::filename_to_uri(input, None)?)
        .build()?;
    let mux = gst::ElementFactory::make("mp4mux")
        .property("faststart", true)
        .build()?;
    let sink = gst::ElementFactory::make("filesink")
        .property("location", output)
        .build()?;
    pipeline.add_many([&dec, &mux, &sink])?;
    mux.link(&sink)?;

    // Video: decoded frames → encoder → mux.
    let vqueue = gst::ElementFactory::make("queue").build()?;
    let mut vchain = vec![
        vqueue.clone(),
        gst::ElementFactory::make("videoconvert").build()?,
    ];
    vchain.extend(encoder.upload_chain()?);
    vchain.push(encoder.build(cfg)?);
    vchain.push(
        gst::ElementFactory::make("capsfilter")
            .property("caps", cfg.codec.caps())
            .build()?,
    );
    vchain.push(cfg.codec.make_parser()?);
    pipeline.add_many(&vchain)?;
    gst::Element::link_many(&vchain)?;
    vchain.last().unwrap().link(&mux)?;

    // Audio: every source track → volume → one mixer → AAC → mux. Only built
    // when there's audio; a mixer with no inputs would never finish.
    let mixer = if audio_tracks > 0 {
        let mixer = gst::ElementFactory::make("audiomixer").build()?;
        let chain = [
            mixer.clone(),
            gst::ElementFactory::make("audioconvert").build()?,
            gst::ElementFactory::make("audioresample").build()?,
            gst::ElementFactory::make("capsfilter")
                .property(
                    "caps",
                    gst::Caps::builder("audio/x-raw")
                        .field("rate", 48_000i32)
                        .field("channels", 2i32)
                        .build(),
                )
                .build()?,
            aac_encoder()?,
            gst::ElementFactory::make("aacparse").build()?,
        ];
        pipeline.add_many(&chain)?;
        gst::Element::link_many(&chain)?;
        chain[5].link(&mux)?;
        Some(mixer)
    } else {
        None
    };

    let link_error: Arc<Mutex<Option<String>>> = Arc::default();
    {
        let pipeline = pipeline.downgrade();
        let volumes = edit.volumes.clone();
        let link_error = link_error.clone();
        let next_audio = Mutex::new(0usize);
        dec.connect_pad_added(move |_, pad| {
            let Some(pipeline) = pipeline.upgrade() else {
                return;
            };
            let caps = pad.current_caps().unwrap_or_else(|| pad.query_caps(None));
            let Some(name) = caps.structure(0).map(|s| s.name().to_string()) else {
                return;
            };
            let result = (|| -> Result<()> {
                if name.starts_with("video/") {
                    pad.link(&vqueue.static_pad("sink").unwrap())?;
                } else if name.starts_with("audio/")
                    && let Some(mixer) = &mixer
                {
                    let i = {
                        let mut n = next_audio.lock().unwrap();
                        *n += 1;
                        *n - 1
                    };
                    let gain = volumes.get(i).copied().unwrap_or(1.0).clamp(0.0, 4.0);
                    let chain = [
                        gst::ElementFactory::make("queue").build()?,
                        gst::ElementFactory::make("audioconvert").build()?,
                        gst::ElementFactory::make("audioresample").build()?,
                        gst::ElementFactory::make("volume")
                            .property("volume", gain)
                            .build()?,
                    ];
                    pipeline.add_many(&chain)?;
                    gst::Element::link_many(&chain)?;
                    chain[3].link(mixer)?;
                    for e in &chain {
                        e.sync_state_with_parent()?;
                    }
                    pad.link(&chain[0].static_pad("sink").unwrap())?;
                }
                Ok(())
            })();
            if let Err(e) = result {
                *link_error.lock().unwrap() = Some(format!("{e:#}"));
            }
        });
    }

    let result = (|| -> Result<()> {
        pipeline.set_state(gst::State::Paused)?;
        wait(&pipeline, gst::MessageType::AsyncDone)?;
        pipeline.seek(
            1.0,
            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
            gst::SeekType::Set,
            edit.start,
            gst::SeekType::Set,
            edit.end,
        )?;
        wait(&pipeline, gst::MessageType::AsyncDone)?;
        pipeline.set_state(gst::State::Playing)?;
        wait(&pipeline, gst::MessageType::Eos)
    })();
    let _ = pipeline.set_state(gst::State::Null);
    let result = match link_error.lock().unwrap().take() {
        Some(e) => result.and(Err(anyhow::anyhow!(e))),
        None => result,
    };
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}

fn wait(pipeline: &gst::Pipeline, until: gst::MessageType) -> Result<()> {
    let bus = pipeline.bus().context("no bus")?;
    match bus.timed_pop_filtered(
        gst::ClockTime::from_seconds(600),
        &[until, gst::MessageType::Error],
    ) {
        Some(msg) => match msg.view() {
            gst::MessageView::Error(e) => bail!(
                "{} from {:?}: {:?}",
                e.error(),
                msg.src().map(|s| s.path_string()),
                e.debug()
            ),
            _ => Ok(()),
        },
        None => bail!("timed out rendering"),
    }
}

fn aac_encoder() -> Result<gst::Element> {
    let name = ["fdkaacenc", "avenc_aac"]
        .into_iter()
        .find(|n| gst::ElementFactory::find(n).is_some())
        .context("no AAC encoder found")?;
    let el = gst::ElementFactory::make(name).build()?;
    el.set_property("bitrate", AAC_BITRATE);
    Ok(el)
}
