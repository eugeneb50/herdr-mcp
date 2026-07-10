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
pub use pfc1::{CompressionKey, CompressionStats, compress_text, decompress_text};
pub use caveman::{CavemanLevel, CavemanResult, compress as caveman_compress};
pub use pipeline::{StageSpec, PipelineResult, StageResult};
pub use runner::{PipelineRunner, MEMORY_FILE};
pub use policy::{TrimPolicy, TrimDirection};
pub use stats::{TrimStats, PaneStat, load_stats, save_stats};
pub use folder_key::{
    FolderKey, FolderKeyOptions, FolderKeyStats,
    build_folder_key, load_folder_key, save_folder_key,
    save_central_key, list_central_keys,
    load_master_key, learn_into_master,
    decompress_with_folder_key, discover_folder_keys,
    FOLDER_KEY_FILE, MASTER_KEY_FILE, CENTRAL_DIR,
};
pub use eval::{trim_eval, trim_bench};
