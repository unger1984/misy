//! Asynchronous model-catalog refresh coordination for the terminal client.

use super::{
    browser::BrowserHandoff,
    client::{ProviderOperationResult, TuiClient},
};
use crate::AvailableModels;
use std::thread;

impl<B: BrowserHandoff> TuiClient<B> {
    pub(super) fn start_model_refresh(&mut self) {
        self.open_cached_model_picker();
        self.model_refresh_generation = self.model_refresh_generation.wrapping_add(1);
        let generation = self.model_refresh_generation;
        let core = self.core.clone();
        let sender = self.operation_sender.clone();
        thread::spawn(move || {
            let result = core.available_models().map_err(|error| error.to_string());
            let _ = sender.send(ProviderOperationResult::Models(generation, result));
        });
    }

    fn open_cached_model_picker(&mut self) {
        match self.core.cached_available_models() {
            Ok(available) => self.state.open_models(available),
            Err(error) => {
                self.state.add_error(error);
                self.state.open_models(AvailableModels::default());
            }
        }
    }
}
