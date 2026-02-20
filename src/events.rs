use crossterm::event::Event;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickControl {
    Start,       // Start sending tick events
    Stop,        // Stop sending tick events
    PauseInput,  // Stop polling stdin (for external editor)
    ResumeInput, // Resume polling stdin
}

#[derive(Debug)]
pub enum AppEvent {
    Input(Event),
    Tick,
    Disconnect,                               // Sent when SSH connection is disconnected
    TerminalUpdate,                           // Sent when SSH terminal receives data
    SftpProgress(crate::transfer::ScpResult), // Sent when SFTP transfer has progress/completion
}
