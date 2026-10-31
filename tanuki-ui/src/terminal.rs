//! Own terminal state for normal return, I/O errors and Rust unwinding.
use crate::{
    app::{App, AppError, ConnectionConfig},
    ui::{Action, ViewState},
};
use crossterm::{
    cursor::{Hide, Show},
    event::{Event, EventStream},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io::{self, Stdout, Write};

#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error("terminal I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Session(#[from] AppError),
}
struct Restore;
impl Drop for Restore {
    fn drop(&mut self) {
        // Attempt every restoration step even if one fails, including partial
        // initialization. No panic catching or global panic-hook replacement.
        let raw = disable_raw_mode();
        let screen = restore_screen(&mut io::stdout());
        if let Err(error) = raw.and(screen) {
            tracing::warn!(?error, "terminal restoration failed");
        }
    }
}
fn restore_screen(writer: &mut impl Write) -> io::Result<()> {
    let screen = execute!(writer, LeaveAlternateScreen);
    let cursor = execute!(writer, Show);
    screen.and(cursor)
}
pub struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    _restore: Restore,
}
impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let restore = Restore;
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        let terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        Ok(Self {
            terminal,
            _restore: restore,
        })
    }
}

pub async fn run(config: ConnectionConfig) -> Result<(), TerminalError> {
    let mut terminal = TerminalGuard::enter()?;
    let mut app = App::new(config);
    let result = event_loop(&mut terminal, &mut app).await;
    // Restore the shell before waiting on any network cleanup or printing errors.
    drop(terminal);
    let closed = app.close().await;
    result?;
    closed?;
    Ok(())
}
async fn event_loop(terminal: &mut TerminalGuard, app: &mut App) -> io::Result<()> {
    let mut view = ViewState::default();
    let mut input = EventStream::new();
    let mut ticks = tokio::time::interval(std::time::Duration::from_millis(100));
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    let mut dirty = true;
    app.begin_reconnect();
    loop {
        tokio::select! {
            result=&mut interrupt=>{result?;break;},
            _=app.next_update()=>{dirty=true;}, // Typed failures are already visible in model status.
            event=input.next()=>match event.transpose()? {
                Some(Event::Key(key))=>{
                    match view.key(key,app.model_mut()) {
                        Action::Quit=>break,
                        Action::Reconnect=>app.begin_reconnect(),
                        Action::Continue=>{},
                    }
                    dirty=true;
                },
                Some(Event::Resize(..))=>dirty=true,
                Some(_)=>{},
                None=>break,
            },
            _=ticks.tick()=>if dirty {
                terminal.terminal.draw(|frame|view.draw(frame,app.model(),app.config()))?;
                dirty=false;
            },
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_restoration_is_attempted_even_when_screen_restoration_fails() {
        struct FailOnce {
            failed: bool,
            bytes: Vec<u8>,
        }
        impl Write for FailOnce {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if !self.failed {
                    self.failed = true;
                    Err(io::Error::other("screen write failed"))
                } else {
                    self.bytes.extend_from_slice(bytes);
                    Ok(bytes.len())
                }
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut writer = FailOnce {
            failed: false,
            bytes: vec![],
        };
        assert!(restore_screen(&mut writer).is_err());
        assert!(String::from_utf8(writer.bytes).unwrap().contains("?25h"));
    }
}
