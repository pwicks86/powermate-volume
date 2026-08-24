#![windows_subsystem = "windows"]

mod windows_debugger;

use clap::Parser;

use windows_debugger::LOGGER;

use log::{self, LevelFilter};
use tokio::time::sleep;
use tray_icon::{
    Icon, TrayIconBuilder,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem},
};
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

use anyhow::Result;
use futures_lite::future;
use hidapi::HidApi;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Sender;
use windows::{
    Win32::{
        Media::Audio::{
            Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole,
            eRender,
        },
        System::Com::{
            CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
        },
        UI::Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
            VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK,
        },
    },
    core::Interface,
};
use winit::{
    application::ApplicationHandler,
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
};

use tokio::sync::mpsc;

// PowerMate VID and PID
const VID: u16 = 0x077d;
const PID: u16 = 0x0410;

const APP_NAME: &str = "PowerMate Volume";

/// Current state of the default playback endpoint. Sent to the tray thread
/// after every volume change so the icon's tooltip reflects it.
#[derive(Debug, Clone, Copy)]
struct VolumeState {
    level: f32,
    muted: bool,
}

impl VolumeState {
    fn tooltip(&self) -> String {
        if self.muted {
            format!("{APP_NAME} — Muted")
        } else {
            format!("{APP_NAME} — {}%", (self.level * 100.0).round() as u32)
        }
    }
}

#[derive(Debug)]
enum HidEvent {
    Press,
    Release,
    Turn(i8),
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
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Enable verbose output
    #[arg(short, long)]
    verbose: bool,
}

fn setup_logging(verbose: bool) {
    log::set_logger(&LOGGER).unwrap();
    let level_str = if verbose {
        log::set_max_level(LevelFilter::Debug);
        "debug"
    } else {
        log::set_max_level(LevelFilter::Info);
        "info"
    };
    log::info!("Log level set to {}", level_str);
}

// #[tokio::main]
fn main() {
    let args = Args::parse();
    setup_logging(args.verbose);
    log::info!("starting app");

    // The event loop has to exist before the tokio side starts, so we can hand
    // the background tasks a proxy to push tooltip updates back to the tray.
    let event_loop = EventLoop::<VolumeState>::with_user_event().build().unwrap();
    let proxy = event_loop.create_proxy();

    // start tokio in background thread
    let (tx_app, rx_app) = tokio::sync::mpsc::channel(64);
    let (tx_cmd, rx_cmd) = tokio::sync::mpsc::channel(64);

    let tray_tx_cmd = tx_cmd.clone();

    std::thread::spawn(move || {
        tokio_runtime(tx_app, tx_cmd, rx_app, rx_cmd, proxy);
    });

    // 🚨 MAIN THREAD = TRAY
    tray_main_thread(event_loop, tray_tx_cmd);
}

fn tokio_runtime(
    tx_app: Sender<HidEvent>,
    tx_cmd: Sender<CommandEvent>,
    rx_app: tokio::sync::mpsc::Receiver<HidEvent>,
    rx_cmd: tokio::sync::mpsc::Receiver<CommandEvent>,
    proxy: EventLoopProxy<VolumeState>,
) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(async move {
        tokio::spawn(hid_task(tx_app));
        tokio::spawn(fsm_task(rx_app, tx_cmd.clone()));
        tokio::spawn(media_task(rx_cmd, proxy));

        future::pending::<()>().await;
    });
}

async fn hid_task(tx: Sender<HidEvent>) {
    tokio::task::spawn_blocking(move || {
        let mut api = HidApi::new().expect("Failed to create HidApi");

        loop {
            log::info!("Waiting for PowerMate...");

            // ---- WAIT FOR DEVICE ----
            let device_info = loop {
                api.refresh_devices().ok();

                if let Some(dev) = api
                    .device_list()
                    .find(|d| d.vendor_id() == VID && d.product_id() == PID)
                {
                    break dev.clone();
                }

                std::thread::sleep(Duration::from_millis(500));
            };

            log::info!("PowerMate connected!");

            // ---- OPEN DEVICE ----
            let device = match device_info.open_device(&api) {
                Ok(d) => d,
                Err(e) => {
                    log::error!("Failed to open device: {:?}", e);
                    continue;
                }
            };

            if let Err(e) = device.set_blocking_mode(false) {
                log::error!("Failed to set non-blocking mode: {:?}", e);
            }
            let mut last_pressed = false;

            // ---- READ LOOP ----
            loop {
                let mut buf = [0u8; 64];

                match device.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        let cmd_buf: [u8; 6] = match buf[..6].try_into() {
                            Ok(b) => b,
                            Err(_) => continue,
                        };

                        let pressed = cmd_buf[0] != 0;
                        let delta = cmd_buf[1] as i8;

                        if delta == 0 {
                            if pressed && !last_pressed {
                                tx.blocking_send(HidEvent::Press).ok();
                            } else if !pressed && last_pressed {
                                tx.blocking_send(HidEvent::Release).ok();
                            }
                            last_pressed = pressed;
                        } else {
                            tx.blocking_send(HidEvent::Turn(delta)).ok();
                        }
                    }

                    Ok(_) => {
                        // no data
                        std::thread::sleep(Duration::from_millis(5));
                    }

                    Err(e) => {
                        log::warn!("Device disconnected or read error: {:?}", e);
                        break; // 🔥 exit read loop → go back to wait
                    }
                }
            }

            log::info!("PowerMate disconnected");
        }
    })
    .await
    .unwrap();
}

async fn fsm_task(mut rx: mpsc::Receiver<HidEvent>, tx_cmd: mpsc::Sender<CommandEvent>) {
    let click_window = Duration::from_millis(350);
    let long_press_threshold = Duration::from_millis(600);

    let mut click_count = 0;
    let mut last_release = Instant::now();
    let mut press_start: Option<Instant> = None;

    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                match event {
                    HidEvent::Press => {
                        press_start = Some(Instant::now());
                    }

                    HidEvent::Release => {
                        let now = Instant::now();

                        // ---- LONG PRESS CHECK ----
                        if let Some(start) = press_start.take()
                            && now.duration_since(start) >= long_press_threshold
                        {
                            click_count = 0; // cancel click sequence
                            let _ = tx_cmd.send(CommandEvent::Mute).await; // long press action
                            continue;
                        }

                        // ---- MULTI CLICK LOGIC ----
                        if now.duration_since(last_release) <= click_window {
                            click_count += 1;
                        } else {
                            click_count = 1;
                        }

                        last_release = now;
                    }

                    HidEvent::Turn(d) => {
                        let cmd = if d > 0 {
                            CommandEvent::VolumeUp
                        } else {
                            CommandEvent::VolumeDown
                        };
                        let _ = tx_cmd.send(cmd).await;
                    }
                }
            }

            // ---- CLICK RESOLUTION TIMER ----
            _ = sleep(click_window), if click_count > 0 => {
                let cmd = match click_count {
                    1 => CommandEvent::PlayPause,
                    2 => CommandEvent::Next,
                    3 => CommandEvent::Prev,
                    _ => CommandEvent::PlayPause,
                };

                let _ = tx_cmd.send(cmd).await;

                click_count = 0;
            }
        }
    }
}

async fn media_task(mut rx: mpsc::Receiver<CommandEvent>, proxy: EventLoopProxy<VolumeState>) {
    // Seed the tooltip with the current state, so it's accurate before the
    // knob is ever touched. All COM work stays on this thread — see the note
    // in with_endpoint_volume about initializing COM on the tray thread.
    match read_volume_state() {
        Ok(state) => {
            let _ = proxy.send_event(state);
        }
        Err(e) => log::error!("Failed to read initial volume state: {:?}", e),
    }

    while let Some(cmd) = rx.recv().await {
        // Only the volume commands change endpoint state; the media-key ones
        // just forward a keystroke and leave the tooltip alone.
        let new_state = match cmd {
            CommandEvent::VolumeUp => Some(change_volume(0.025)),
            CommandEvent::VolumeDown => Some(change_volume(-0.025)),
            CommandEvent::Mute => {
                log::info!("Mute");
                Some(toggle_mute())
            }
            CommandEvent::PlayPause => {
                log::info!("PlayPause");
                send_play_pause();
                None
            }
            CommandEvent::Next => {
                log::info!("Next");
                send_next_track();
                None
            }
            CommandEvent::Prev => {
                log::info!("Prev");
                send_prev_track();
                None
            }
        };

        match new_state {
            Some(Ok(state)) => {
                let _ = proxy.send_event(state);
            }
            Some(Err(e)) => log::error!("Volume command failed: {:?}", e),
            None => {}
        }
    }
}

const AUTOSTART_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const AUTOSTART_VALUE_NAME: &str = "PowerMateVolume";

fn is_autostart_enabled() -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(run_key) = hkcu.open_subkey(AUTOSTART_KEY_PATH) else {
        return false;
    };
    run_key.get_value::<String, _>(AUTOSTART_VALUE_NAME).is_ok()
}

fn set_autostart(enabled: bool) -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (run_key, _) = hkcu.create_subkey(AUTOSTART_KEY_PATH)?;

    if enabled {
        let exe_path = std::env::current_exe()?;
        let value = format!("\"{}\"", exe_path.display());
        run_key.set_value(AUTOSTART_VALUE_NAME, &value)?;
        log::info!("Enabled start with Windows ({})", value);
    } else {
        match run_key.delete_value(AUTOSTART_VALUE_NAME) {
            Ok(()) => log::info!("Disabled start with Windows"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }

    Ok(())
}

struct TrayApp {
    tx_cmd: Sender<CommandEvent>,
    tray: Option<tray_icon::TrayIcon>,
    quit_id: Option<tray_icon::menu::MenuId>,
    play_id: Option<tray_icon::menu::MenuId>,
    start_id: Option<tray_icon::menu::MenuId>,
    start_item: Option<CheckMenuItem>,
    /// Latest state from the media task. Kept so the tooltip can be restored
    /// if an update arrives before the tray icon has been built.
    volume_state: Option<VolumeState>,
}

impl TrayApp {
    fn new(tx_cmd: Sender<CommandEvent>) -> Self {
        Self {
            tx_cmd,
            tray: None,
            quit_id: None,
            play_id: None,
            start_id: None,
            start_item: None,
            volume_state: None,
        }
    }

    fn tooltip(&self) -> String {
        match self.volume_state {
            Some(state) => state.tooltip(),
            None => APP_NAME.to_string(),
        }
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

impl ApplicationHandler<VolumeState> for TrayApp {
    fn resumed(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
        log::info!("tray resumed called");
        let play = MenuItem::new("Play/Pause", true, None);
        let start_with_windows =
            CheckMenuItem::new("Start with Windows", true, is_autostart_enabled(), None);
        let quit = MenuItem::new("Quit", true, None);

        self.play_id = Some(play.id().clone());
        self.start_id = Some(start_with_windows.id().clone());
        self.start_item = Some(start_with_windows.clone());
        self.quit_id = Some(quit.id().clone());

        let menu = Menu::new();
        menu.append(&play).unwrap();
        menu.append(&start_with_windows).unwrap();
        menu.append(&quit).unwrap();

        let icon = load_icon();

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(icon)
            // Windows shows a tooltip window on hover whether or not there's
            // text for it; without this it renders as an empty box.
            .with_tooltip(self.tooltip())
            .build()
            .unwrap();

        self.tray = Some(tray);
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, state: VolumeState) {
        self.volume_state = Some(state);

        if let Some(tray) = &self.tray
            && let Err(e) = tray.set_tooltip(Some(state.tooltip()))
        {
            log::error!("Failed to update tray tooltip: {:?}", e);
        }
    }

    fn about_to_wait(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let menu_rx = MenuEvent::receiver();

        while let Ok(ev) = menu_rx.try_recv() {
            if Some(ev.id.clone()) == self.quit_id {
                event_loop.exit();
            } else if Some(ev.id.clone()) == self.play_id {
                self.tx_cmd.blocking_send(CommandEvent::PlayPause).ok();
            } else if Some(ev.id.clone()) == self.start_id
                && let Some(item) = &self.start_item
            {
                let enabled = item.is_checked();
                if let Err(e) = set_autostart(enabled) {
                    log::error!("Failed to update start with Windows setting: {:?}", e);
                    item.set_checked(!enabled);
                }
            }
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        _event: winit::event::WindowEvent,
    ) {
    }
}

fn tray_main_thread(event_loop: EventLoop<VolumeState>, tx_cmd: Sender<CommandEvent>) {
    log::info!("tray_main_thread");

    let mut app = TrayApp::new(tx_cmd);

    log::info!("about to run app");
    event_loop.run_app(&mut app).unwrap_or_else(|e| {
        log::error!("run_app failed: {:?}", e);
    });
}

fn with_endpoint_volume<F, T>(f: F) -> Result<T>
where
    F: FnOnce(&IAudioEndpointVolume) -> Result<T>,
{
    unsafe fn open_endpoint() -> Result<IAudioEndpointVolume> {
        unsafe {
            // Get audio endpoint enumerator
            let device_enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

            // Get default playback device
            let device = device_enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;

            // Activate endpoint volume interface
            Ok(device
                .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)?
                .cast()?)
        }
    }

    unsafe {
        // S_OK/S_FALSE both mean this thread now owes a CoUninitialize;
        // RPC_E_CHANGED_MODE means COM was already up in a different mode and
        // this call did nothing, so uninitializing would unbalance someone
        // else's init. Anything fallible below goes through open_endpoint so
        // an early `?` can't skip the matching CoUninitialize.
        let owns_com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();

        let result = open_endpoint().and_then(|volume| f(&volume));

        if owns_com {
            CoUninitialize();
        }
        result
    }
}

fn read_volume_state() -> Result<VolumeState> {
    with_endpoint_volume(|ep| unsafe {
        Ok(VolumeState {
            level: ep.GetMasterVolumeLevelScalar()?,
            muted: ep.GetMute()?.as_bool(),
        })
    })
}

fn change_volume(change_amt: f32) -> Result<VolumeState> {
    with_endpoint_volume(|ep| unsafe {
        let cur_level = ep.GetMasterVolumeLevelScalar()?;
        let level = (cur_level + change_amt).clamp(0.0, 1.0);
        ep.SetMasterVolumeLevelScalar(level, std::ptr::null())?;
        Ok(VolumeState {
            level,
            muted: ep.GetMute()?.as_bool(),
        })
    })
}

fn toggle_mute() -> Result<VolumeState> {
    with_endpoint_volume(|ep| unsafe {
        let muted = ep.GetMute()?.as_bool();
        log::info!("mute_status = {}", muted);
        ep.SetMute(!muted, std::ptr::null())?;
        Ok(VolumeState {
            level: ep.GetMasterVolumeLevelScalar()?,
            muted: !muted,
        })
    })
}

fn send_key(key_code: VIRTUAL_KEY) {
    unsafe {
        // key down
        let down = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: key_code,
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
                    wVk: key_code,
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };

        let inputs = [down, up];

        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn send_play_pause() {
    send_key(VK_MEDIA_PLAY_PAUSE);
}

fn send_next_track() {
    send_key(VK_MEDIA_NEXT_TRACK);
}

fn send_prev_track() {
    send_key(VK_MEDIA_PREV_TRACK);
}
