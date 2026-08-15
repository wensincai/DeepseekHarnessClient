//! dsh-client — a thin desktop shell for DeepSeek Harness.
//!
//! Responsibilities:
//!   1. Spawn `scripts/dsh-server.mjs` (build-if-stale + `dsh web`) as a child.
//!   2. Show a loading window while the server comes up.
//!   3. Navigate the embedded WebView to `http://127.0.0.1:3080` when ready.
//!   4. Kill the child process tree on exit.

mod server;

use tauri::RunEvent;

#[tauri::command]
fn server_status(state: tauri::State<'_, server::ServerState>) -> server::ServerStatusInfo {
    server::status(&state)
}

pub fn run() {
    tauri::Builder::default()
        .manage(server::ServerState::default())
        .invoke_handler(tauri::generate_handler![server_status])
        .setup(|app| {
            server::start(app.handle().clone());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building dsh-client")
        .run(|app_handle, event| {
            if let RunEvent::Exit = event {
                server::stop(app_handle);
            }
        });
}
