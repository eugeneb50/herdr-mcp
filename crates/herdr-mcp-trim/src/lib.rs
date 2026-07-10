pub mod caveman;
pub mod code_regions;
pub mod dashboard;
pub mod eval;
pub mod folder_key;
pub mod pfc1;
pub mod pipeline;
pub mod policy;
pub mod runner;
pub mod stats;

// Selective re-exports of the key public types (no glob `use X::*`)
pub use caveman::{CavemanLevel, CavemanResult, compress as caveman_compress};
pub use eval::{trim_bench, trim_eval};
pub use folder_key::{
    CENTRAL_DIR, FOLDER_KEY_FILE, FolderKey, FolderKeyOptions, FolderKeyStats, MASTER_KEY_FILE,
    build_folder_key, decompress_with_folder_key, discover_folder_keys, learn_into_master,
    list_central_keys, load_folder_key, load_master_key, save_central_key, save_folder_key,
};
pub use pfc1::{CompressionKey, CompressionStats, compress_text, decompress_text};
pub use pipeline::{PipelineResult, StageResult, StageSpec};
pub use policy::{TrimDirection, TrimPolicy};
pub use runner::{MEMORY_FILE, PipelineRunner};
pub use stats::{PaneStat, TrimStats, load_stats, save_stats};
