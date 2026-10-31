//! Example application assembling the local RoomKit adapter.

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match roomkit_tui_examples::engine::launch().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("RoomKit example failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
