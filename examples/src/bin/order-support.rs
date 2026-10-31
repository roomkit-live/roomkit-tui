//! Run a fictional order-support application using the shared terminal.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    roomkit_tui_examples::launch(roomkit_tui_examples::Profile::Orders)
}
