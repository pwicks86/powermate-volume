#![windows_subsystem = "windows"]

mod windows_debugger;

use windows_debugger::LOGGER;
use ctrlc;

use std::{sync::{Arc, atomic::{AtomicBool, Ordering}}, thread};
use log::{self, LevelFilter};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    TrayIcon, TrayIconBuilder,
    Icon
};

use winit::{
    application::ApplicationHandler,
    event_loop::{ActiveEventLoop, EventLoop},
};
use hidapi::HidApi;
use anyhow::{Result};
use std::time::{Duration, Instant};
use windows::{Win32::{Media::Audio::{Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender}, System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize}, UI::Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY}}, core::Interface};
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, PlatformConfig};



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
        log::debug!("Handling report, pressed = {}, delta = {}", pressed, delta);

        if delta == 0 {
            log::debug!("Button Event");
            if self.last_pressed {
                events.push(PowerMateEvent::Release);
                self.last_pressed = false;
            } else {
                events.push(PowerMateEvent::Press);
                self.last_pressed = true;
            }

        } else {

            if self.last_pressed {
                events.push(PowerMateEvent::TurnWhilePressed(delta));
            } else {
                events.push(PowerMateEvent::Turn(delta));
            }

        }
        events
    }
}

fn load_icon() -> Icon {
    let bytes = include_bytes!("icon.png");

    // Load image from disk
    let img = image::load_from_memory(bytes)
        .expect("Failed to load icon")
        .into_rgba8();


    let (width, height) = img.dimensions();
    let rgba = img.into_raw();

    Icon::from_rgba(rgba, width, height).expect("Failed to create icon")
}
struct App {
    tray_icon: Option<TrayIcon>,
    quit_id: Option<tray_icon::menu::MenuId>,
    // should_exit: Arc<AtomicBool>,

}

impl ApplicationHandler for App {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {
        // Build menu
        let quit = MenuItem::new("Quit", true, None);
        self.quit_id = Some(quit.id().clone());

        // let quit = MenuItem::new("Quit", true, None);
        // self.quit_id = Some(quit.id().clone());

        let menu = Menu::new();
        menu.append(&quit).unwrap();
        let icon = load_icon();

        // Create tray icon

        let tray_icon = TrayIconBuilder::new()
            .with_tooltip("Powermate Control")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .build()
            .unwrap();

        self.tray_icon = Some(tray_icon);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {

        // Handle menu events
        let menu_channel = MenuEvent::receiver();

        while let Ok(event) = menu_channel.try_recv() {
            log::info!("about to wait");
            if let Some(ref quit_id) = self.quit_id {
                if event.id == *quit_id {
                    log::info!("Quitting...");
                    event_loop.exit();
                }
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        todo!()
    }
}

fn setup_logging() {
    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(LevelFilter::Debug);

}

fn main() {
    setup_logging();
    // log::info!("Main started");
    let event_loop = EventLoop::new().unwrap();
    // let should_exit = Arc::new(AtomicBool::new(false));
    // let flag = should_exit.clone();

    // ctrlc::set_handler(move || {
    //     flag.store(true, Ordering::Relaxed);
    // })
    // .expect("Error setting Ctrl-C handler");
    thread::spawn(volume_thread);

    let mut app = App {
        tray_icon: None,
        quit_id: None,
        // should_exit
    };

    event_loop.run_app(&mut app).unwrap();
}

fn volume_thread() {

    let api = HidApi::new().expect("Failed to create HidApi");

    // List devices
    for device in api.device_list() {
        log::info!(
            "VID: {:04x}, PID: {:04x}, Path: {:?}",
            device.vendor_id(),
            device.product_id(),
            device.path()
        );
    }

    let device = api.open(VID, PID).expect("Failed to open powermate");
    device.set_blocking_mode(false);

    let mut pmate = PowerMate::new();

    loop{
        let mut buf = [0u8; 64];
        match device.read(&mut buf) {
            Ok(n) if n > 0 => {
                log::debug!("Read {} bytes: {:?}", n, &buf[..n]);
                if n >= 6 {
                    let first_six = buf[..6].try_into().expect("failed to get first six bytes");
                    let events = pmate.handle_report(first_six);
                    for event in events {
                        dispatch_event(event);
                    }
                } else {
                    log::warn!("Invalid number of bytes");
                }
            }
            _ => {}
        }
    }
}


fn with_endpoint_volume<F, T>(f: F) -> Result<T>
where
    F: FnOnce(&IAudioEndpointVolume) -> Result<T>,
{
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
const VK_MEDIA_PLAY_PAUSE: u16 = 0xB3;

fn send_play_pause() {
    unsafe {
        // key down
        let down = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(VK_MEDIA_PLAY_PAUSE),
                    wScan: 0,
                    dwFlags: Default::default(),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };

        // key up
        let up = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(VK_MEDIA_PLAY_PAUSE),
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };

        let inputs = [down, up];
        log::info!("Sending play/pause");

        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn dispatch_event(evt: PowerMateEvent) {
    match evt {
        PowerMateEvent::Press => {
            log::info!("Press");
        },
        PowerMateEvent::Release => {
            // toggle_mute().expect("woo");
            log::info!("Release");
            send_play_pause();
        },
        PowerMateEvent::Turn(val) => {
            if val > 0 {
                log::info!("CW");
                change_volume(0.025).expect("woo");
            } else {
                log::info!("CCW");
                change_volume(-0.025).expect("woo");
            }
        },
        PowerMateEvent::TurnWhilePressed(_) => {

        }
    }

}
