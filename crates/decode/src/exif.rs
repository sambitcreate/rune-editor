//! Camera EXIF metadata extraction (`{Exif}` and `{TIFF}` dictionaries).
//!
//! On macOS, extracts metadata natively via Apple's `ImageIO`
//! (`CGImageSourceCopyPropertiesAtIndex`) with zero extra crate dependencies,
//! supporting Camera RAW (DNG/CR3/NEF/ARW/RAF), ProRAW, HEIC, JPEG, TIFF, PNG,
//! and in-memory image buffers. On other platforms, falls back gracefully.

use std::path::Path;

/// UI-friendly camera EXIF metadata extracted from a photograph.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhotoExif {
    pub camera_model: Option<String>,
    pub lens_model: Option<String>,
    pub focal_length_mm: Option<f32>,
    pub aperture_f: Option<f32>,
    /// Human-readable shutter speed (e.g. `"1/250s"`, `"0.5s"`, `"2s"`).
    pub shutter_display: Option<String>,
    pub iso: Option<u32>,
    pub date_taken: Option<String>,
}

impl PhotoExif {
    /// Returns `true` when no EXIF fields were populated.
    pub fn is_empty(&self) -> bool {
        self.camera_model.is_none()
            && self.lens_model.is_none()
            && self.focal_length_mm.is_none()
            && self.aperture_f.is_none()
            && self.shutter_display.is_none()
            && self.iso.is_none()
            && self.date_taken.is_none()
    }

    /// Formats the non-empty exposure parameters as a compact summary line
    /// (e.g. `"50mm • f/2.0 • 1/250s • ISO 400"`).
    pub fn exposure_summary(&self) -> String {
        let mut parts = Vec::with_capacity(4);

        if let Some(fl) = self.focal_length_mm
            && fl.is_finite()
            && fl > 0.0
            && fl <= 100_000.0
        {
            if (fl - fl.round()).abs() < 0.05 {
                parts.push(format!("{}mm", fl.round() as u32));
            } else {
                parts.push(format!("{fl:.1}mm"));
            }
        }

        if let Some(fnum) = self.aperture_f
            && fnum.is_finite()
            && fnum > 0.0
        {
            parts.push(format!("f/{fnum:.1}"));
        }

        if let Some(shutter) = self.shutter_display.as_deref() {
            let trimmed = shutter.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_owned());
            }
        }

        if let Some(iso) = self.iso
            && iso > 0
        {
            parts.push(format!("ISO {iso}"));
        }

        parts.join(" • ")
    }
}

/// Formats an EXIF exposure duration in seconds into photographic notation
/// (e.g. `0.004 -> "1/250s"`, `0.5 -> "0.5s"`, `2.0 -> "2s"`).
pub fn format_shutter_speed(seconds: f64) -> Option<String> {
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    if seconds >= 1.0 {
        if (seconds - seconds.round()).abs() < 0.05 && seconds <= 1_000_000.0 {
            Some(format!("{}s", seconds.round() as u64))
        } else {
            Some(format!("{seconds:.1}s"))
        }
    } else if seconds >= 0.4 {
        Some(format!("{seconds:.1}s"))
    } else {
        let denom = (1.0 / seconds).round();
        if denom.is_finite() && (1.0..=1_000_000.0).contains(&denom) {
            Some(format!("1/{}s", denom as u64))
        } else {
            None
        }
    }
}

/// Extracts [`PhotoExif`] metadata from an image file on disk.
///
/// Returns `None` if the file cannot be inspected or contains no supported EXIF/TIFF tags.
pub fn extract_exif_from_path(path: &Path) -> Option<PhotoExif> {
    #[cfg(target_os = "macos")]
    {
        macos::extract_exif_from_path(path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        None
    }
}

/// Extracts [`PhotoExif`] metadata from in-memory image bytes (e.g. JPEG, TIFF, HEIC, DNG).
///
/// Returns `None` if the bytes contain no supported EXIF/TIFF tags.
pub fn extract_exif_from_bytes(bytes: &[u8]) -> Option<PhotoExif> {
    if bytes.is_empty() {
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        macos::extract_exif_from_bytes(bytes)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = bytes;
        None
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;
    use std::path::Path;

    use objc2::rc::autoreleasepool;
    use objc2_core_foundation::{CFArray, CFData, CFDictionary, CFNumber, CFString, CFType, CFURL};
    use objc2_image_io::{
        CGImageSource, kCGImagePropertyExifDateTimeOriginal, kCGImagePropertyExifDictionary,
        kCGImagePropertyExifExposureTime, kCGImagePropertyExifFNumber,
        kCGImagePropertyExifFocalLength, kCGImagePropertyExifISOSpeedRatings,
        kCGImagePropertyExifLensModel, kCGImagePropertyTIFFDictionary, kCGImagePropertyTIFFModel,
    };

    use super::{PhotoExif, format_shutter_speed};

    pub(super) fn extract_exif_from_path(path: &Path) -> Option<PhotoExif> {
        let abs_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        };

        autoreleasepool(|_| {
            let cf_url = CFURL::from_file_path(&abs_path)?;
            let source = unsafe { CGImageSource::with_url(&cf_url, None) }?;
            extract_from_source(&source)
        })
    }

    pub(super) fn extract_exif_from_bytes(bytes: &[u8]) -> Option<PhotoExif> {
        autoreleasepool(|_| {
            let cf_data = CFData::from_bytes(bytes);
            let source = unsafe { CGImageSource::with_data(&cf_data, None) }?;
            extract_from_source(&source)
        })
    }

    fn extract_from_source(source: &CGImageSource) -> Option<PhotoExif> {
        let props = unsafe { source.properties_at_index(0, None) }?;

        let tiff = dict_get_dict(&props, unsafe { kCGImagePropertyTIFFDictionary });
        let exif = dict_get_dict(&props, unsafe { kCGImagePropertyExifDictionary });

        let camera_model =
            tiff.and_then(|d| dict_get_string(d, unsafe { kCGImagePropertyTIFFModel }));
        let lens_model =
            exif.and_then(|d| dict_get_string(d, unsafe { kCGImagePropertyExifLensModel }));
        let focal_length_mm = exif
            .and_then(|d| dict_get_f64(d, unsafe { kCGImagePropertyExifFocalLength }))
            .filter(|&v| v > 0.0)
            .map(|v| v as f32);
        let aperture_f = exif
            .and_then(|d| dict_get_f64(d, unsafe { kCGImagePropertyExifFNumber }))
            .filter(|&v| v > 0.0)
            .map(|v| v as f32);
        let shutter_display = exif
            .and_then(|d| dict_get_f64(d, unsafe { kCGImagePropertyExifExposureTime }))
            .and_then(format_shutter_speed);
        let iso =
            exif.and_then(|d| dict_get_iso(d, unsafe { kCGImagePropertyExifISOSpeedRatings }));
        let date_taken =
            exif.and_then(|d| dict_get_string(d, unsafe { kCGImagePropertyExifDateTimeOriginal }));

        let parsed = PhotoExif {
            camera_model,
            lens_model,
            focal_length_mm,
            aperture_f,
            shutter_display,
            iso,
            date_taken,
        };

        if parsed.is_empty() {
            None
        } else {
            Some(parsed)
        }
    }

    fn dict_get_raw<'a>(dict: &'a CFDictionary, key: &CFString) -> Option<&'a CFType> {
        let key_ptr: *const CFString = key;
        let val = unsafe { dict.value(key_ptr.cast::<c_void>()) };
        if val.is_null() {
            None
        } else {
            // SAFETY: ImageIO property dictionaries hold CFType values borrowed for `dict`'s lifetime.
            Some(unsafe { &*val.cast::<CFType>() })
        }
    }

    fn dict_get_dict<'a>(dict: &'a CFDictionary, key: &CFString) -> Option<&'a CFDictionary> {
        dict_get_raw(dict, key)?.downcast_ref::<CFDictionary>()
    }

    fn dict_get_string(dict: &CFDictionary, key: &CFString) -> Option<String> {
        let raw = dict_get_raw(dict, key)?.downcast_ref::<CFString>()?;
        let trimmed = raw.to_string().trim().to_owned();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }

    fn dict_get_f64(dict: &CFDictionary, key: &CFString) -> Option<f64> {
        dict_get_raw(dict, key)?
            .downcast_ref::<CFNumber>()?
            .as_f64()
            .filter(|v| v.is_finite())
    }

    fn dict_get_iso(dict: &CFDictionary, key: &CFString) -> Option<u32> {
        let raw = dict_get_raw(dict, key)?;
        let number = if let Some(arr) = raw.downcast_ref::<CFArray>() {
            if arr.count() <= 0 {
                return None;
            }
            let first = unsafe { arr.value_at_index(0) };
            if first.is_null() {
                return None;
            }
            // SAFETY: Elements of ImageIO's ISOSpeedRatings CFArray are CFType values.
            let first_cf = unsafe { &*first.cast::<CFType>() };
            first_cf.downcast_ref::<CFNumber>()?
        } else {
            raw.downcast_ref::<CFNumber>()?
        };
        let v = number.as_i64()?;
        if v > 0 && v <= u32::MAX as i64 {
            Some(v as u32)
        } else {
            None
        }
    }
}
