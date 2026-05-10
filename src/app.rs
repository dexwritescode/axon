use tokio::sync::mpsc;

use crate::{config::Config, event::AppEvent};

pub struct App {
    pub running: bool,
    pub config: Config,
    /// Receives AppEvent variants from the active inference task.
    /// Replace to cancel the previous task and start a new one.
    pub inference_rx: Option<mpsc::Receiver<AppEvent>>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            running: true,
            config: Config::default(),
            inference_rx: None,
        }
    }
}
