#![windows_subsystem = "windows"]

mod windows_debugger;

use windows_debugger::LOGGER;
use ctrlc;

use tokio::time::{sleep};
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
// use std::sync::mpsc::{Sender};
use tokio::sync::mpsc::{Sender};
use futures_lite::future;

use tokio::sync::mpsc;

// PowerMate VID and PID
const VID: u16 = 0x077d;
const PID: u16 = 0x0410;

#[derive(Debug)]
enum HidEvent {
    Press,
    Release,
    Turn(i8),
}

#[derive(Debug)]
enum AppEvent {
    Hid(HidEvent),
    Command(CommandEvent),
}

#[derive(Debug)]
enum CommandEvent {
    PlayPause,
    Next,
    Prev,
    VolumeUp,
    VolumeDown,
    Mute,
}


fn setup_logging() {
    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(LevelFilter::Debug);

}

// #[tokio::main]
fn main() {
    setup_logging();
    log::info!("starting app");

    // start tokio in background thread
    let (tx_app, rx_app) = tokio::sync::mpsc::channel(64);
    let (tx_cmd, rx_cmd) = tokio::sync::mpsc::channel(64);

    std::thread::spawn(move || {
        tokio_runtime(tx_app, tx_cmd, rx_app, rx_cmd);
    });

    // 🚨 MAIN THREAD = TRAY
    tray_main_thread();
}

fn tokio_runtime(
    tx_app: Sender<AppEvent>,
    tx_cmd: Sender<CommandEvent>,
    mut rx_app: tokio::sync::mpsc::Receiver<AppEvent>,
    mut rx_cmd: tokio::sync::mpsc::Receiver<CommandEvent>,
) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(async move {
        tokio::spawn(hid_task(tx_app));
        tokio::spawn(fsm_task(rx_app, tx_cmd.clone()));
        tokio::spawn(media_task(rx_cmd));

        future::pending::<()>().await;
    });

}

async fn hid_task(tx: Sender<AppEvent>) {
    tokio::task::spawn_blocking(move || {

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
        let mut last_pressed = false;

        // let mut pmate = PowerMate::new();
        loop {

            let mut buf = [0u8; 64];
            match device.read(&mut buf) {
                Ok(n) => {
                    if n > 0 {
                        let cmd_buf: [u8; 6] = buf[..6].try_into().expect("failed to get first six bytes");
                        let pressed = cmd_buf[0] != 0;
                        let delta = cmd_buf[1] as i8;
                        if delta == 0 {
                            if !last_pressed {
                                tx.blocking_send(AppEvent::Hid(HidEvent::Press)).ok();
                                last_pressed = true;
                            } else {
                                tx.blocking_send(AppEvent::Hid(HidEvent::Release)).ok();
                                last_pressed = false;

                            }
                        } else {
                            tx.blocking_send(AppEvent::Hid(HidEvent::Turn(delta))).ok();
                        }
                    } else {
                        // log::warn!("Invalid number of bytes: {}", n);
                    }
                }
                _ => {}
            }
        }
    }).await.unwrap();
}


async fn fsm_task(
    mut rx: mpsc::Receiver<AppEvent>,
    tx_cmd: mpsc::Sender<CommandEvent>,
) {
    let mut click_count = 0;
    let mut last_click = Instant::now();
    let window = Duration::from_millis(350);

    loop {
        if let Some(AppEvent::Hid(event)) = rx.recv().await {
            match event {
                HidEvent::Turn(d) => {
                    if d > 0 {
                        let _ = tx_cmd.send(CommandEvent::VolumeUp).await;
                    } else {
                        let _ = tx_cmd.send(CommandEvent::VolumeDown).await;
                    }
                }

                HidEvent::Press => {}

                HidEvent::Release => {
                    let now = Instant::now();

                    if now.duration_since(last_click) <= window {
                        click_count += 1;
                    } else {
                        click_count = 1;
                    }

                    last_click = now;

                    let tx = tx_cmd.clone();
                    let count = click_count;

                    tokio::spawn(async move {
                        sleep(window).await;

                        let cmd = match count {
                            1 => CommandEvent::PlayPause,
                            2 => CommandEvent::Mute,
                            3 => CommandEvent::Next,
                            _ => CommandEvent::PlayPause,
                        };

                        let _ = tx.send(cmd).await;
                    });
                }
            }
        }
    }
}

use tokio::sync::mpsc::Receiver;

async fn media_task(mut rx: Receiver<CommandEvent>) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            CommandEvent::VolumeUp => volume_up(),
            CommandEvent::VolumeDown => volume_down(),
            _ => {

            }
            // CommandEvent::PlayPause => send_play_pause(),
            // CommandEvent::Next => send_next(),
            // CommandEvent::Prev => send_prev(),
            // CommandEvent::VolumeUp => volume_up(),
            // CommandEvent::VolumeDown => volume_down(),
            // CommandEvent::Mute => toggle_mute(),
        }
    }
}

struct TrayApp {
    // tx_cmd: tokio::sync::mpsc::Sender<CommandEvent>,
    tray: Option<tray_icon::TrayIcon>,
    quit_id: Option<tray_icon::menu::MenuId>,
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

impl ApplicationHandler for TrayApp {
    fn resumed(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
        log::info!("tray resumed called");
        let play = MenuItem::new("Play/Pause", true, None);
        let quit = MenuItem::new("Quit", true, None);

        self.quit_id = Some(quit.id().clone());

        let menu = Menu::new();
        menu.append(&play).unwrap();
        menu.append(&quit).unwrap();

        let icon = load_icon();

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(icon)
            .build()
            .unwrap();

        self.tray = Some(tray);
    }

    fn about_to_wait(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let menu_rx = MenuEvent::receiver();

        while let Ok(ev) = menu_rx.try_recv() {
            if Some(ev.id.clone()) == self.quit_id {
                event_loop.exit();
            } else {
            }
        }
    }
    
    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        // todo!()
    }
}

async fn tray_task(tx_cmd: Sender<CommandEvent>) {
    let event_loop = EventLoop::new().unwrap();

    let mut app = TrayApp {
        // tx_cmd,
        quit_id: None,
        tray: None,
    };

    event_loop.run_app(&mut app).unwrap();
}


fn tray_main_thread() {
    log::info!("tray_main_thread");
    let event_loop = EventLoop::new().unwrap();

    let mut app = TrayApp {
        // tx_cmd,
        quit_id: None,
        tray: None,
    };

    log::info!("about to run app");
    event_loop.run_app(&mut app).unwrap_or_else(|e| {
        log::error!("run_app failed: {:?}", e);
    });
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

fn volume_up() {
    change_volume(0.025);
}
fn volume_down() {
    change_volume(-0.025);
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

