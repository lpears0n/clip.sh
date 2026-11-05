//! Live levels for Settings → Audio: one small pipeline per audio source,
//! opening the same device the recorder would (`source → audioconvert →
//! level → fakesink`), reporting the loudest channel's peak about fifteen
//! times a second. It runs only while the page is open, never during a
//! recording's hot path, and a source that can't be opened reports why
//! instead of a level.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;

use crate::audio::AudioSource;

/// How often each source reports.
const INTERVAL: Duration = Duration::from_millis(66);

/// One report for source `index` (in the order given to [`Meter::start`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Reading {
    /// Loudest channel's peak over the last interval, in dBFS (0 = full
    /// scale, silence is very negative).
    Level { index: usize, peak_db: f64 },
    /// The source couldn't be opened or stopped working.
    Failed { index: usize, error: String },
}

/// Running meters; dropping it stops them.
pub struct Meter {
    pipelines: Vec<gst::Pipeline>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Meter {
    /// Meter `sources`, calling `report` from a background thread.
    pub fn start(
        sources: &[AudioSource],
        report: impl Fn(Reading) + Send + 'static,
    ) -> Result<Self> {
        let mut built = Vec::with_capacity(sources.len());
        for source in sources {
            built.push(crate::audio::meter_source(source));
        }
        Self::start_with(built, report)
    }

    /// Meter already-made source elements; an `Err` stands for a source
    /// that couldn't be made and is reported as failed straight away.
    pub fn start_with(
        sources: Vec<Result<gst::Element>>,
        report: impl Fn(Reading) + Send + 'static,
    ) -> Result<Self> {
        let mut pipelines = Vec::new();
        let mut buses = Vec::new();
        for (index, src) in sources.into_iter().enumerate() {
            let built = src.and_then(|src| {
                let pipeline = gst::Pipeline::new();
                let convert = gst::ElementFactory::make("audioconvert").build()?;
                let level = gst::ElementFactory::make("level")
                    .property("interval", INTERVAL.as_nanos() as u64)
                    .property("post-messages", true)
                    .build()
                    .context("level element missing (gst-plugins-good)")?;
                let sink = gst::ElementFactory::make("fakesink")
                    .property("sync", false)
                    .build()?;
                let chain = [src, convert, level, sink];
                pipeline.add_many(&chain)?;
                gst::Element::link_many(&chain)?;
                pipeline
                    .set_state(gst::State::Playing)
                    .context("can't open the device")?;
                Ok(pipeline)
            });
            match built {
                Ok(p) => {
                    buses.push((index, p.bus().context("pipeline has no bus")?));
                    pipelines.push(p);
                }
                Err(e) => report(Reading::Failed {
                    index,
                    error: format!("{e:#}"),
                }),
            }
        }

        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name("reruns-meter".into())
            .spawn(move || {
                let mut failed = vec![false; buses.len()];
                while !stop2.load(Ordering::Relaxed) {
                    let mut idle = true;
                    for (slot, (index, bus)) in buses.iter().enumerate() {
                        while let Some(msg) = bus.pop() {
                            idle = false;
                            if let Some(r) = reading(*index, &msg)
                                && !failed[slot]
                            {
                                failed[slot] = matches!(r, Reading::Failed { .. });
                                report(r);
                            }
                        }
                    }
                    if idle {
                        std::thread::sleep(Duration::from_millis(15));
                    }
                }
            })?;
        Ok(Self {
            pipelines,
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Meter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for p in &self.pipelines {
            let _ = p.set_state(gst::State::Null);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The reading a bus message carries, if any.
fn reading(index: usize, msg: &gst::Message) -> Option<Reading> {
    match msg.view() {
        gst::MessageView::Element(e) => {
            let s = e.structure()?;
            if s.name() != "level" {
                return None;
            }
            let peaks = s.get::<gst::glib::ValueArray>("peak").ok()?;
            let peak_db = peaks
                .iter()
                .filter_map(|v| v.get::<f64>().ok())
                .fold(f64::NEG_INFINITY, f64::max);
            Some(Reading::Level { index, peak_db })
        }
        gst::MessageView::Error(e) => Some(Reading::Failed {
            index,
            error: e.error().to_string(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    fn tone(volume: f64) -> Result<gst::Element> {
        Ok(gst::ElementFactory::make("audiotestsrc")
            .property("is-live", true)
            .property("volume", volume)
            .build()?)
    }

    #[test]
    fn reports_levels_per_source_and_failures() {
        crate::init().unwrap();
        let readings = Arc::new(Mutex::new(Vec::new()));
        let r2 = readings.clone();
        let meter = Meter::start_with(
            vec![tone(0.5), Err(anyhow::anyhow!("no such device")), tone(0.0)],
            move |r| r2.lock().unwrap().push(r),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(600));
        drop(meter);
        let readings = readings.lock().unwrap();

        let peaks = |i: usize| -> Vec<f64> {
            readings
                .iter()
                .filter_map(|r| match r {
                    Reading::Level { index, peak_db } if *index == i => Some(*peak_db),
                    _ => None,
                })
                .collect()
        };
        // ~15 a second for half a second and more.
        assert!(peaks(0).len() >= 5, "{readings:?}");
        // A 0.5 sine peaks near -6 dBFS; silence is far below.
        assert!(
            peaks(0).iter().all(|p| (-7.0..-5.0).contains(p)),
            "{:?}",
            peaks(0)
        );
        assert!(peaks(2).iter().all(|p| *p < -80.0), "{:?}", peaks(2));
        assert!(readings.iter().any(|r| matches!(
            r,
            Reading::Failed { index: 1, error } if error.contains("no such device")
        )));
    }
}
