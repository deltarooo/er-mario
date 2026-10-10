//! Plays libsm64's audio (32 kHz stereo s16) through the default output device.
//!
//! The libsm64 thread pushes samples after each tick; a cpal output stream pulls them and
//! resamples linearly to the device rate.

use std::collections::VecDeque;
use std::sync::Mutex;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::log;

pub const SM64_RATE: f32 = 32_000.0;
const MAX_QUEUED_FRAMES: usize = 6000;

static RING: Mutex<VecDeque<[i16; 2]>> = Mutex::new(VecDeque::new());

/// Left/right gains for positional audio, stored as f32 bits (set from the game thread).
static GAIN_L: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f35_04f3); // ~0.707
static GAIN_R: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f35_04f3);

/// Places Mario's voice: `pan` -1 (left) .. 1 (right), `volume` 0..1. Equal-power panning.
pub fn set_position(pan: f32, volume: f32) {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    let v = volume.clamp(0.0, 1.0);
    GAIN_L.store((angle.cos() * v).to_bits(), std::sync::atomic::Ordering::Relaxed);
    GAIN_R.store((angle.sin() * v).to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// Frames waiting to be played (libsm64 uses this to decide how much to generate).
pub fn queued() -> u32 {
    RING.lock().map(|r| r.len() as u32).unwrap_or(0)
}

pub fn push(samples: &[i16]) {
    let Ok(mut r) = RING.lock() else { return };
    if r.len() > MAX_QUEUED_FRAMES {
        return;
    }
    r.extend(samples.chunks_exact(2).map(|c| [c[0], c[1]]));
}

/// set by the stream's error callback: the device went away under it
static BROKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Opens a stream on the default output device.
fn open(host: &cpal::Host) -> Option<cpal::Stream> {
    let Some(device) = host.default_output_device() else {
        log("audio: no output device");
        return None;
    };
    let Ok(config) = device.default_output_config() else {
        log("audio: no output config");
        return None;
    };
    let rate = config.sample_rate().0 as f32;
    let channels = config.channels() as usize;
    log(format!("audio: output {} Hz, {channels} ch, {:?}", rate, config.sample_format()));
    let step = SM64_RATE / rate;
    let mut pos = 0.0f32;
    let mut cur = [0i16; 2];
    let mut next = [0i16; 2];
    let stream = device.build_output_stream(
        &config.into(),
        move |out: &mut [f32], _| {
            let mut ring = RING.lock().unwrap_or_else(|e| e.into_inner());
            let gl = f32::from_bits(GAIN_L.load(std::sync::atomic::Ordering::Relaxed));
            let gr = f32::from_bits(GAIN_R.load(std::sync::atomic::Ordering::Relaxed));
            for frame in out.chunks_mut(channels) {
                pos += step;
                while pos >= 1.0 {
                    pos -= 1.0;
                    cur = next;
                    next = ring.pop_front().unwrap_or([0, 0]);
                }
                let l = cur[0] as f32 + (next[0] as f32 - cur[0] as f32) * pos;
                let r = cur[1] as f32 + (next[1] as f32 - cur[1] as f32) * pos;
                // fold SM64's (mostly centred) mix to mono and place it where Mario is
                let mono = (l + r) * 0.5 / 32768.0 * std::f32::consts::SQRT_2;
                for (c, s) in frame.iter_mut().enumerate() {
                    *s = mono * if c % 2 == 0 { gl } else { gr };
                }
            }
        },
        |e| {
            log(format!("audio stream error: {e}"));
            BROKEN.store(true, std::sync::atomic::Ordering::Relaxed);
        },
        None,
    );
    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            log(format!("audio: could not open stream: {e}"));
            return None;
        }
    };
    if let Err(e) = stream.play() {
        log(format!("audio: play failed: {e}"));
        return None;
    }
    log("audio: stream started");
    Some(stream)
}

/// Starts the output stream on its own thread (the stream must stay alive). The thread keeps
/// an eye on it: a stream stays on the device it was opened on, so when the player switches
/// to another one (headset plugged in, another output picked in Windows) Mario went silent
/// until the game was restarted. It's opened again on the new default device.
pub fn start() {
    std::thread::spawn(|| {
        let host = cpal::default_host();
        let name = |host: &cpal::Host| host.default_output_device().and_then(|d| d.name().ok());
        // (the device last tried, so one that can't be opened isn't tried every 2 s)
        let mut tried = name(&host);
        let mut playing = open(&host);
        loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let now = name(&host);
            let broken = BROKEN.swap(false, std::sync::atomic::Ordering::Relaxed);
            if broken || (now.is_some() && now != tried) {
                log("audio: the output device changed, opening the stream again");
                drop(playing.take());
                tried = now;
                playing = open(&host);
            }
        }
    });
}
