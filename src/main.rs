use log;
use hidapi::HidApi;
use anyhow::{anyhow, Result};
use std::time::{Duration, Instant};
use windows::{Win32::{Media::Audio::{Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender}, System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize}}, core::Interface};


// PowerMate VID and PID
const VID: u16 = 0x077d;
const PID: u16 = 0x0410;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PowerMateEvent {
    Press,
    Release,
    Turn(i8),
    TurnWhilePressed(i8),
}

pub struct PowerMate {
    last_pressed: bool,
    last_event_time: Instant,
    debounce: Duration,
}

impl PowerMate {
    pub fn new() -> Self {
        Self {
            last_pressed: false,
            last_event_time: Instant::now(),
            debounce: Duration::from_millis(50),
        }
    }

    pub fn handle_report(&mut self, buf: [u8; 6]) -> Vec<PowerMateEvent> {
        let mut events = Vec::new();

        let pressed = buf[0] != 0;
        let delta = buf[1] as i8;
        let now = Instant::now();

        // --- Handle button transitions (deduplicated) ---
        if pressed != self.last_pressed {
            if pressed {
                events.push(PowerMateEvent::Press);
            } else {
                events.push(PowerMateEvent::Release);
            }
            self.last_pressed = pressed;
            self.last_event_time = now;
        }

        // --- Handle rotation ---
        if delta != 0 {
            // If we somehow missed a press but we're rotating while "pressed",
            // optionally infer it (helps with very fast clicks + turns)
            if pressed && !self.last_pressed {
                if now.duration_since(self.last_event_time) < self.debounce {
                    events.push(PowerMateEvent::Press);
                    self.last_pressed = true;
                }
            }

            if pressed {
                events.push(PowerMateEvent::TurnWhilePressed(delta));
            } else {
                events.push(PowerMateEvent::Turn(delta));
            }

            self.last_event_time = now;
        }

        events
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init(); // Initialize the logger at the start
    let api = HidApi::new()?;

    // List devices
    for device in api.device_list() {
        log::info!(
            "VID: {:04x}, PID: {:04x}, Path: {:?}",
            device.vendor_id(),
            device.product_id(),
            device.path()
        );
    }

    let device = api.open(VID, PID)?;
    device.set_blocking_mode(false);

    let mut pmate = PowerMate::new();

    loop{
        let mut buf = [0u8; 64];
        match device.read(&mut buf) {
            Ok(n) if n > 0 => {
                log::debug!("Read {} bytes: {:?}", n, &buf[..n]);
                if n >= 6 {
                    let first_six = buf[..6].try_into()?;
                    let events = pmate.handle_report(first_six);
                    for event in events {
                        dispatch_event(event);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

// enum PowerMateEvent {
//     ClickIn,
//     ClickOut,
//     RotateRight,
//     RotateLeft,
//     ClickedRotateRight,
//     ClickedRotateLeft,
//     Invalid
// }

fn with_endpoint_volume<F, T>(f: F) -> Result<T>
where
    F: FnOnce(&IAudioEndpointVolume) -> Result<T>,
{
// fn windows_volume_junk() -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED);

        // Get audio endpoint enumerator
        let device_enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

        // Get default playback device
        let device = device_enumerator.GetDefaultAudioEndpoint(
            eRender,
            eConsole,
        )?;

        // Activate endpoint volume interface
        let volume: IAudioEndpointVolume = device.Activate::<IAudioEndpointVolume>(
                CLSCTX_ALL,
                None,
            )?
            .cast()?;

        let result = f(&volume);

        CoUninitialize();
        result
    }
}

fn change_volume(change_amt: f32) -> Result<()> {
    with_endpoint_volume(|ep| {
        unsafe {
            let cur_level = ep.GetMasterVolumeLevelScalar()?;
            let level = (cur_level + change_amt).clamp(0.0, 1.0);
            ep.SetMasterVolumeLevelScalar(level, std::ptr::null())?;
        }
        Ok(())
    })
}

fn toggle_mute() -> Result<()> {
    with_endpoint_volume(|ep| {
        unsafe {
            let mute_status = ep.GetMute();
            log::info!("mute_status = {}", mute_status.as_ref().unwrap().as_bool());
            match mute_status {
                Ok(muted) => {

                    ep.SetMute(!muted.as_bool(),  std::ptr::null());
                }
                Err(e) => {
                    log::error!("Error changing volume");
                },
            }
        }
        Ok(())
    })
}

fn dispatch_event(evt: PowerMateEvent) {
    match evt {
        PowerMateEvent::Press => {

        },
        PowerMateEvent::Release => {
            toggle_mute().expect("woo");
        },
        PowerMateEvent::Turn(val) => {
            if val > 0 {
                change_volume(0.025).expect("woo");
            } else {
                change_volume(-0.025).expect("woo");
            }
        },
        PowerMateEvent::TurnWhilePressed(_) => {

        }
    }

}
