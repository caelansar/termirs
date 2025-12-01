use crossterm::event::Event;

#[derive(Debug)]
pub enum AppEvent {
    Input(Event),
    Tick,
    Disconnect,
}
