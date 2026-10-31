//! Run the generic scripted terminal host.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    roomkit_tui_examples::launch(roomkit_tui_examples::Profile::Assistant)
}
