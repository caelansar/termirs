use crossterm::event::Event;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickControl {
    Start, // Start sending tick events
    Stop,  // Stop sending tick events
}

#[derive(Debug)]
pub enum AppEvent {
    Input(Event),
    Tick,
    Disconnect,     // Sent when SSH connection is disconnected
    TerminalUpdate, // Sent when SSH terminal receives data
}
