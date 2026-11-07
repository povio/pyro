//! The process's system fonts, shared by page rendering (lp-render) and
//! canvas text: the system font scan (fontique, tens of ms) runs once, on
//! first use, and every user works on a clone. Clones share the scanned system
//! fonts and the loaded font data; fonts a user registers stay its own.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};

use parley::fontique::{Collection, CollectionOptions, SourceCache};
use parley::FontContext;

static FONTS: LazyLock<Mutex<FontContext>> = LazyLock::new(|| {
    Mutex::new(FontContext {
        source_cache: SourceCache::new_shared(),
        collection: if FORK_SAFE_FONTS.load(Ordering::Relaxed) {
            fork_safe_collection()
        } else {
            Collection::new(CollectionOptions { shared: false, system_fonts: true })
        },
    })
});

static FORK_SAFE_FONTS: AtomicBool = AtomicBool::new(false);

/// A clone of the process's font context.
pub fn font_context() -> FontContext {
    FONTS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Loads system fonts without the platform font API, now, on this thread
/// (`serve --fork`). macOS's CoreText starts dispatch threads and Objective-C
/// initializers that a later `fork()` cannot survive ("+[NSFontCollection
/// initialize] may have been in progress"); a fork template, and children
/// that fork again, must not use it. Fonts are read from the system font
/// directories instead (memory-mapped), with the generic families and
/// script fallbacks the platform backend would pick. Call before any font use.
pub fn use_fork_safe_fonts() {
    FORK_SAFE_FONTS.store(true, Ordering::Relaxed);
    drop(font_context());
}

/// Whether [`use_fork_safe_fonts`] was called (fonts never touch CoreText).
pub fn fork_safe_fonts() -> bool {
    FORK_SAFE_FONTS.load(Ordering::Relaxed)
}

/// The system fonts by directory scan: see [`use_fork_safe_fonts`].
fn fork_safe_collection() -> Collection {
    use parley::fontique::{GenericFamily, Script};
    let mut c = Collection::new(CollectionOptions { shared: false, system_fonts: false });
    let mut dirs: Vec<std::path::PathBuf> = if cfg!(target_os = "macos") {
        ["/System/Library/Fonts", "/Library/Fonts"].iter().map(Into::into).collect()
    } else {
        ["/usr/share/fonts", "/usr/local/share/fonts"].iter().map(Into::into).collect()
    };
    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::PathBuf::from(home);
        dirs.push(if cfg!(target_os = "macos") { home.join("Library/Fonts") } else { home.join(".local/share/fonts") });
    }
    c.load_fonts_from_paths(dirs.iter().filter(|d| d.is_dir()));
    let ids = |c: &mut Collection, names: &[&str]| -> Vec<parley::fontique::FamilyId> {
        names.iter().filter_map(|n| c.family_id(n)).collect()
    };
    // As fontique's CoreText backend (Chromium's macOS defaults), then
    // common Linux families.
    let generics: &[(GenericFamily, &[&str])] = &[
        (GenericFamily::Serif, &["Times", "Times New Roman", "DejaVu Serif", "Liberation Serif"]),
        (GenericFamily::SansSerif, &["Helvetica", "Arial", "DejaVu Sans", "Liberation Sans"]),
        (GenericFamily::Monospace, &["Courier", "Courier New", "Menlo", "DejaVu Sans Mono", "Liberation Mono"]),
        (GenericFamily::Cursive, &["Apple Chancery"]),
        (GenericFamily::Fantasy, &["Papyrus"]),
        (GenericFamily::SystemUi, &["System Font", ".SF NS", "SF Pro", "Helvetica Neue", "Helvetica", "DejaVu Sans"]),
        (GenericFamily::UiSansSerif, &["System Font", ".SF NS", "SF Pro", "Helvetica Neue", "Helvetica"]),
        (GenericFamily::UiSerif, &["New York", "Times"]),
        (GenericFamily::UiMonospace, &["SF Mono", "Menlo", "Courier"]),
        (GenericFamily::Emoji, &["Apple Color Emoji", "Noto Color Emoji"]),
        (GenericFamily::Math, &["STIX Two Math"]),
    ];
    for (generic, names) in generics {
        let families = ids(&mut c, names);
        c.set_generic_families(*generic, families.into_iter());
    }
    // Script fallbacks CoreText would answer for the usual scripts.
    let fallbacks: &[(&[u8; 4], &[&str])] = &[
        (b"Latn", &["Helvetica", "Lucida Grande", "Arial Unicode MS", "DejaVu Sans"]),
        (b"Grek", &["Helvetica", "Lucida Grande", "Arial Unicode MS", "DejaVu Sans"]),
        (b"Cyrl", &["Helvetica", "Lucida Grande", "Arial Unicode MS", "DejaVu Sans"]),
        (b"Hani", &["PingFang SC", "Heiti SC", "Hiragino Sans GB", "Noto Sans CJK SC"]),
        (b"Hans", &["PingFang SC", "Heiti SC", "Hiragino Sans GB", "Noto Sans CJK SC"]),
        (b"Hant", &["PingFang TC", "Heiti TC", "Noto Sans CJK TC"]),
        (b"Jpan", &["Hiragino Sans", "Hiragino Kaku Gothic ProN", "Noto Sans CJK JP"]),
        (b"Hira", &["Hiragino Sans", "Hiragino Kaku Gothic ProN", "Noto Sans CJK JP"]),
        (b"Kana", &["Hiragino Sans", "Hiragino Kaku Gothic ProN", "Noto Sans CJK JP"]),
        (b"Hang", &["Apple SD Gothic Neo", "Noto Sans CJK KR"]),
        (b"Kore", &["Apple SD Gothic Neo", "Noto Sans CJK KR"]),
        (b"Arab", &["Geeza Pro", "Noto Sans Arabic", "DejaVu Sans"]),
        (b"Hebr", &["Arial Hebrew", "Noto Sans Hebrew", "DejaVu Sans"]),
        (b"Thai", &["Thonburi", "Noto Sans Thai"]),
        (b"Deva", &["Kohinoor Devanagari", "Devanagari Sangam MN", "Noto Sans Devanagari"]),
        (b"Zsym", &["Apple Symbols", "Apple Color Emoji", "Noto Color Emoji"]),
        (b"Zsye", &["Apple Color Emoji", "Noto Color Emoji"]),
    ];
    for (script, names) in fallbacks {
        let families = ids(&mut c, names);
        c.set_fallbacks(Script::from_bytes(**script), families.into_iter());
    }
    c
}
