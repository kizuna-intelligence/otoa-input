mod audio;
pub mod console;
mod dotenv;
pub mod instance_lock;
#[cfg(target_os = "linux")]
mod overlay_hints;
mod paths;
mod resample;
mod textout;

pub use audio::{AudioCapture, AudioDevice, AudioFrame, FRAME_SAMPLES};
pub use dotenv::{load_from_ancestors, parse};
#[cfg(target_os = "linux")]
pub use overlay_hints::{apply_overlay_hints, primary_screen_size};
#[cfg(not(target_os = "linux"))]
pub fn primary_screen_size() -> Option<(f64, f64)> {
    None
}
pub use paths::{data_directory, set_app_directory, settings_path};
pub use resample::Resampler;
pub use textout::{PasteMethod, TextOutput};
