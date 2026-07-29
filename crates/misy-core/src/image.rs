//! Validated, bounded image payloads shared by submissions and local tools.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageFormat, ImageReader, imageops::FilterType};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use std::{
    error::Error,
    fmt,
    fs::File,
    io::{Cursor, Read},
    sync::Arc,
};

/// Maximum number of images accepted with one user submission.
pub const MAX_SUBMISSION_IMAGES: usize = 4;
/// Maximum encoded source size accepted before image decoding.
pub const MAX_IMAGE_INPUT_BYTES: usize = 20 * 1024 * 1024;
/// Maximum normalized PNG size placed on the provider wire.
pub const MAX_IMAGE_OUTPUT_BYTES: usize = 5 * 1024 * 1024;
/// Maximum normalized image bytes retained for one active provider request.
pub const MAX_ACTIVE_IMAGE_BYTES: usize = 20 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 1_568;
const MAX_DECODED_IMAGE_BYTES: u64 = 128 * 1024 * 1024;

/// Input modality advertised by a provider model.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputModality {
    /// UTF-8 text input.
    Text,
    /// Raster image input.
    Image,
}

/// Metadata safe to expose to clients without copying an image payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageAttachmentInfo {
    /// Normalized media type.
    pub media_type: String,
    /// Normalized image width in pixels.
    pub width: u32,
    /// Normalized image height in pixels.
    pub height: u32,
    /// Normalized encoded size in bytes.
    pub bytes: usize,
}

/// A validated image whose immutable bytes can be shared across queue and history snapshots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageAttachment {
    bytes: Arc<[u8]>,
    width: u32,
    height: u32,
}

impl ImageAttachment {
    /// Decodes PNG, JPEG, GIF, or WebP bytes and normalizes the first frame to a bounded PNG.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported, malformed, oversized, or excessively large images.
    pub fn from_encoded(bytes: impl AsRef<[u8]>) -> Result<Self, ImageError> {
        let bytes = bytes.as_ref();
        if bytes.len() > MAX_IMAGE_INPUT_BYTES {
            return Err(ImageError::InputTooLarge(bytes.len()));
        }
        let format = image::guess_format(bytes).map_err(|_| ImageError::UnsupportedFormat)?;
        if !matches!(
            format,
            ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP
        ) {
            return Err(ImageError::UnsupportedFormat);
        }
        let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(MAX_DECODED_IMAGE_BYTES);
        limits.max_image_width = Some(MAX_IMAGE_DIMENSION.saturating_mul(16));
        limits.max_image_height = Some(MAX_IMAGE_DIMENSION.saturating_mul(16));
        reader.limits(limits);
        let image = reader.decode().map_err(ImageError::Decode)?;
        Self::normalize(image)
    }

    /// Builds an attachment from clipboard RGBA pixels and normalizes it to PNG.
    ///
    /// # Errors
    ///
    /// Returns an error when dimensions do not match the pixel buffer or output exceeds limits.
    pub fn from_rgba(width: u32, height: u32, bytes: Vec<u8>) -> Result<Self, ImageError> {
        let expected = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(ImageError::InvalidDimensions)?;
        if expected > MAX_DECODED_IMAGE_BYTES || expected != bytes.len() as u64 {
            return Err(ImageError::InvalidDimensions);
        }
        let rgba = image::RgbaImage::from_raw(width, height, bytes)
            .ok_or(ImageError::InvalidDimensions)?;
        Self::normalize(DynamicImage::ImageRgba8(rgba))
    }

    /// Reads and validates an image file using the same policy as clipboard attachments.
    ///
    /// # Errors
    ///
    /// Returns filesystem or image-validation failures.
    pub fn from_path(path: impl AsRef<std::path::Path>) -> Result<Self, ImageError> {
        let path = path.as_ref();
        let metadata = std::fs::metadata(path).map_err(ImageError::Io)?;
        // Blocking-pool tasks cannot be cancelled, so opening a FIFO or device is unsafe.
        if !metadata.is_file() {
            return Err(ImageError::NotRegularFile);
        }
        let file = File::open(path).map_err(ImageError::Io)?;
        let mut bytes = Vec::with_capacity(MAX_IMAGE_INPUT_BYTES.min(64 * 1024));
        file.take(MAX_IMAGE_INPUT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(ImageError::Io)?;
        Self::from_encoded(bytes)
    }

    /// Returns normalized metadata without exposing the payload.
    pub fn info(&self) -> ImageAttachmentInfo {
        ImageAttachmentInfo {
            media_type: self.media_type().to_owned(),
            width: self.width,
            height: self.height,
            bytes: self.bytes.len(),
        }
    }

    /// Returns the normalized MIME type.
    pub fn media_type(&self) -> &'static str {
        "image/png"
    }

    /// Returns normalized PNG bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn normalize(image: DynamicImage) -> Result<Self, ImageError> {
        let image = if image.width() > MAX_IMAGE_DIMENSION || image.height() > MAX_IMAGE_DIMENSION {
            image.resize(
                MAX_IMAGE_DIMENSION,
                MAX_IMAGE_DIMENSION,
                FilterType::Triangle,
            )
        } else {
            image
        };
        let width = image.width();
        let height = image.height();
        let mut encoded = Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, ImageFormat::Png)
            .map_err(ImageError::Encode)?;
        let encoded = encoded.into_inner();
        if encoded.len() > MAX_IMAGE_OUTPUT_BYTES {
            return Err(ImageError::OutputTooLarge(encoded.len()));
        }
        Ok(Self {
            bytes: Arc::from(encoded),
            width,
            height,
        })
    }
}

impl Serialize for ImageAttachment {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct WireImage {
            r#type: &'static str,
            media_type: &'static str,
            data_base64: String,
        }
        WireImage {
            r#type: "image",
            media_type: self.media_type(),
            data_base64: STANDARD.encode(&self.bytes),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ImageAttachment {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireImage {
            r#type: String,
            media_type: String,
            data_base64: String,
        }
        let value = WireImage::deserialize(deserializer)?;
        if value.r#type != "image" || value.media_type != "image/png" {
            return Err(D::Error::custom(
                "attachment must be a normalized PNG image",
            ));
        }
        let bytes = STANDARD
            .decode(value.data_base64)
            .map_err(D::Error::custom)?;
        Self::from_encoded(bytes).map_err(D::Error::custom)
    }
}

/// Image loading and normalization failure.
#[derive(Debug)]
pub enum ImageError {
    /// Source bytes exceed the input limit.
    InputTooLarge(usize),
    /// Normalized PNG exceeds the provider-wire limit.
    OutputTooLarge(usize),
    /// Source format is not supported.
    UnsupportedFormat,
    /// Pixel dimensions do not match the supplied buffer or allocation budget.
    InvalidDimensions,
    /// Reading a local file failed.
    Io(std::io::Error),
    /// The supplied path does not resolve to a regular file.
    NotRegularFile,
    /// Decoding source bytes failed.
    Decode(image::ImageError),
    /// Encoding the normalized PNG failed.
    Encode(image::ImageError),
}

impl fmt::Display for ImageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputTooLarge(size) => write!(formatter, "image is too large ({size} bytes)"),
            Self::OutputTooLarge(size) => {
                write!(formatter, "normalized image is too large ({size} bytes)")
            }
            Self::UnsupportedFormat => formatter.write_str("unsupported image format"),
            Self::InvalidDimensions => formatter.write_str("invalid or excessive image dimensions"),
            Self::Io(error) => write!(formatter, "could not read image: {error}"),
            Self::NotRegularFile => formatter.write_str("image path must be a regular file"),
            Self::Decode(error) => write!(formatter, "could not decode image: {error}"),
            Self::Encode(error) => write!(formatter, "could not encode image: {error}"),
        }
    }
}

impl Error for ImageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Decode(error) | Self::Encode(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_clipboard_pixels_to_png() {
        let image = ImageAttachment::from_rgba(1, 1, vec![255, 0, 0, 255])
            .expect("normalize clipboard pixel");

        assert_eq!(image.media_type(), "image/png");
        assert_eq!((image.info().width, image.info().height), (1, 1));
        assert_eq!(
            image::guess_format(image.bytes()).expect("detect PNG"),
            ImageFormat::Png
        );
    }

    #[test]
    fn rejects_mismatched_clipboard_dimensions() {
        let error = ImageAttachment::from_rgba(2, 2, vec![0; 4])
            .expect_err("invalid pixel buffer must fail");

        assert!(matches!(error, ImageError::InvalidDimensions));
    }

    #[test]
    fn serialized_attachment_contains_only_normalized_wire_fields() {
        let image = ImageAttachment::from_rgba(1, 1, vec![0, 0, 0, 255])
            .expect("normalize clipboard pixel");
        let value = serde_json::to_value(image).expect("serialize attachment");

        assert_eq!(value["type"], "image");
        assert_eq!(value["media_type"], "image/png");
        assert!(
            value["data_base64"]
                .as_str()
                .is_some_and(|data| !data.is_empty())
        );
    }
}
