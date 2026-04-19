use log;
use hidapi::HidApi;
use anyhow::{anyhow, Result};
use windows::Win32::Media::Audio::IMMDevice;
use windows::{Win32::{Media::Audio::{Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender}, System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize}}, core::Interface};


const VID: u16 = 0x077d;
const PID: u16 = 0x0410;

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

    // Open device (replace with your VID/PID)
    // let vid = 0x1234;
    // let pid = 0x5678;

    let device = api.open(VID, PID)?;

    // // Write (send output report)
    // let data = [0x00, 0x01, 0x02, 0x03]; // first byte is report ID (often 0)
    // device.write(&data)?;

    // // Read (input report)
    loop{
        let mut buf = [0u8; 64];
        let len = device.read(&mut buf)?;
        log::debug!("Read {} bytes: {:?}", len, &buf[..len]);
        dispatch_msg(buf);

    }
    Ok(())
}

enum PowerMateEvent {
    ClickIn,
    ClickOut,
    RotateRight,
    RotateLeft,
    ClickedRotateRight,
    ClickedRotateLeft,
    Invalid
}

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

fn dispatch_msg(buf: [u8; 64]) -> Result<()> {
    let event = match buf {
        [0x01, 0x00, ..] => PowerMateEvent::ClickIn,
        [0x00, 0x00, ..] => PowerMateEvent::ClickOut,
        [0x00, 0x01, ..] => PowerMateEvent::RotateRight,
        [0x00, 0xff, ..] => PowerMateEvent::RotateLeft,
        [0x01, 0x01, ..] => PowerMateEvent::ClickedRotateRight,
        [0x01, 0xff, ..] => PowerMateEvent::ClickedRotateLeft,
        [a,b, ..] => PowerMateEvent::Invalid,
        _ => PowerMateEvent::Invalid
    };
    match event {
        PowerMateEvent::ClickIn => {
            log::info!("ClickIn");
        },
        PowerMateEvent::ClickOut => {
            log::info!("ClickOut");
            toggle_mute().expect("woo");
        },
        PowerMateEvent::RotateRight => {
            log::info!("RotateRight");
            change_volume(0.025).expect("woo");
        },
        PowerMateEvent::RotateLeft => {
            log::info!("RotateLeft");
            change_volume(-0.025).expect("woo");
        },
        PowerMateEvent::ClickedRotateRight => {
            println!("ClickedRotateRight");
        },
        PowerMateEvent::ClickedRotateLeft => {
            println!("ClickedRotateLeft");
        },
        PowerMateEvent::Invalid => {
            log::error!("Invalid Event!");
        }
    }
    Ok(())
}