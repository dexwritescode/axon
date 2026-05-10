use crate::config::Config;

pub struct App {
    pub running: bool,
    pub config: Config,
}

impl Default for App {
    fn default() -> Self {
        Self {
            running: true,
            config: Config::default(),
        }
    }
}
