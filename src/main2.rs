use tokio::sync::mpsc;

#[tokio::main]
async fn main() {
    let (tx_app, rx_app) = mpsc::channel::<AppEvent>(64);
    let (tx_cmd, rx_cmd) = mpsc::channel::<CommandEvent>(64);

    // HID (blocking)
    tokio::spawn(hid_task(tx_app.clone()));

    // FSM / gesture engine
    tokio::spawn(fsm_task(rx_app, tx_cmd.clone()));

    // Media controller
    tokio::spawn(media_task(rx_cmd));

    // Tray + UI bridge (runs inside Tokio but owns winit loop)
    tokio::spawn(tray_task(tx_cmd));

    futures::future::pending::<()>().await;
}