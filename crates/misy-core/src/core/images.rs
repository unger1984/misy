//! Capability and model gates for multimodal submissions and tools.

use super::{CoreError, CoreState, MisyCore};
use crate::{
    IMAGE_INPUT_CAPABILITY, IMAGE_INPUT_CAPABILITY_VERSION, ImageAttachment, InputModality,
    MAX_ACTIVE_IMAGE_BYTES, MAX_SUBMISSION_IMAGES, ModelRef,
};

impl MisyCore {
    /// Reports whether the selected model and its provider support one input modality.
    ///
    /// # Panics
    ///
    /// Panics if a prior core task poisoned the selected-model or model-cache mutex.
    pub fn selected_model_supports(&self, modality: InputModality) -> bool {
        let selected = self
            .inner
            .state
            .selected_model
            .lock()
            .expect("selected model mutex must not be poisoned")
            .clone();
        selected.is_some_and(|model| self.inner.state.model_supports(&model, modality))
    }
}

impl CoreState {
    pub(super) fn validate_image_input(
        &self,
        model: &ModelRef,
        attachments: &[ImageAttachment],
    ) -> Result<(), CoreError> {
        if attachments.len() > MAX_SUBMISSION_IMAGES {
            return Err(CoreError::TooManyAttachments {
                found: attachments.len(),
                maximum: MAX_SUBMISSION_IMAGES,
            });
        }
        let bytes = attachments.iter().fold(0_usize, |total, image| {
            total.saturating_add(image.bytes().len())
        });
        if bytes > MAX_ACTIVE_IMAGE_BYTES {
            return Err(CoreError::AttachmentPayloadTooLarge {
                found: bytes,
                maximum: MAX_ACTIVE_IMAGE_BYTES,
            });
        }
        if attachments.is_empty() {
            return Ok(());
        }
        if !self.provider_supports_images(model) {
            return Err(CoreError::UnsupportedCapability {
                provider: model.provider.clone(),
                capability: IMAGE_INPUT_CAPABILITY.to_owned(),
                version: IMAGE_INPUT_CAPABILITY_VERSION,
            });
        }
        if !self.model_supports(model, InputModality::Image) {
            return Err(CoreError::UnsupportedInput {
                model: model.clone(),
                modality: InputModality::Image,
            });
        }
        Ok(())
    }

    pub(super) fn model_supports(&self, model: &ModelRef, modality: InputModality) -> bool {
        let model_supports = self
            .model_cache
            .load()
            .into_iter()
            .find(|candidate| candidate.model == *model)
            .is_some_and(|candidate| candidate.supports(modality));
        model_supports && (modality != InputModality::Image || self.provider_supports_images(model))
    }

    fn provider_supports_images(&self, model: &ModelRef) -> bool {
        self.catalog.get(&model.provider).is_some_and(|package| {
            package
                .manifest()
                .supports_capability(IMAGE_INPUT_CAPABILITY, IMAGE_INPUT_CAPABILITY_VERSION)
        })
    }
}
