//! `@font-face` `unicode-range` (CSS Fonts 4 §4.5): a face is a candidate for a
//! character only if its range includes it, and the faces of one family that
//! match a style equally (a composite font) are checked last rule first.
//! Google Fonts declares one face per script subset (cyrillic, greek, latin,
//! ...) with the same family, weight and style, and each subset's own
//! character map also covers characters other subsets are for: Pyro
//! drew a word's capital A from another subset than the rest of the word.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use pyro_render::*;

use crate::pdf_fidelity::{find, inflated};

/// Inter, cut to `E2E-DUN-MONITOR-OPEN 1,234.56 (EUR) x-y AB xqx` (capitals,
/// six digits, space, x, q, y and punctuation).
const INTER: &[u8] = include_bytes!("fonts/Inter-subset.ttf");
const LATO: &[u8] = include_bytes!("../../../bench/screenshot/fixtures/fonts/Lato-Regular.ttf");

type Done = Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>;

/// Serves `inter.ttf` and `lato.ttf`. With `reversed`, it holds the first
/// font until the second is asked for and delivers them second first, so
/// the faces load in the reverse of their rules' order.
struct Fonts {
    reversed: bool,
    held: Mutex<Option<(Done, Vec<u8>)>>,
}

impl Fetcher for Fonts {
    fn fetch(&self, url: String, done: Done) {
        let font = if url.ends_with("/inter.ttf") {
            INTER
        } else if url.ends_with("/lato.ttf") {
            LATO
        } else {
            return done(Err("offline".into()));
        };
        if !self.reversed {
            return done(Ok(font.to_vec()));
        }
        let mut held = self.held.lock().unwrap();
        match held.take() {
            None => *held = Some((done, font.to_vec())),
            Some((first, first_font)) => {
                drop(held);
                done(Ok(font.to_vec()));
                first(Ok(first_font));
            }
        }
    }
}

/// `text` in 40px `family`, whose faces `faces` declares, printed on a Letter page.
fn print(faces: &str, family: &str, text: &str, reversed: bool) -> PdfOutput {
    let html =
        format!(r#"<!doctype html><style>{faces} body {{ margin: 0; font: 40px {family} }}</style><p>{text}</p>"#);
    let snap = Snapshot {
        html,
        base_url: "http://test/dir/".into(),
        width: 800,
        height: 600,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(Fonts { reversed, held: Mutex::new(None) });
    Renderer::new().print_pdf(&snap, &fetcher, &PdfOptions::new(".pz-page", (816.0, 1056.0))).unwrap()
}

/// The characters each font of the PDF draws (its ToUnicode map's text,
/// sorted), by font name without the subset tag.
fn drawn_by(pdf: &[u8]) -> BTreeMap<String, String> {
    let text = inflated(pdf);
    let mut fonts: BTreeMap<String, Vec<char>> = BTreeMap::new();
    for dict in text.split("/BaseFont").skip(1) {
        let name = dict.trim_start().trim_start_matches('/');
        let name: String = name.chars().take_while(|c| !c.is_whitespace() && !"/<>[]".contains(*c)).collect();
        let name = name.split_once('+').map_or(name.as_str(), |(_, name)| name).to_owned();
        // Type0 fonts name their descendant too, which has no ToUnicode.
        let dict = &dict[..dict.find(">>").unwrap_or(dict.len())];
        let Some(at) = dict.find("/ToUnicode ") else { continue };
        let id: String = dict[at + 11..].chars().take_while(char::is_ascii_digit).collect();
        let obj = find(pdf, format!("\n{id} 0 obj").as_bytes()).expect("the ToUnicode stream");
        let start = find(&pdf[obj..], b"stream\n").unwrap() + obj + 7;
        let end = find(&pdf[start..], b"\nendstream").unwrap() + start;
        let cmap = miniz_oxide::inflate::decompress_to_vec_zlib(&pdf[start..end]).unwrap();
        let cmap = String::from_utf8_lossy(&cmap);
        let chars = fonts.entry(name).or_default();
        for section in cmap.split("beginbfchar").skip(1) {
            let section = section.split("endbfchar").next().unwrap();
            let hex: Vec<&str> = section.split(['<', '>']).map(str::trim).filter(|s| !s.is_empty()).collect();
            for pair in hex.chunks(2) {
                let units: Vec<u16> = pair[1]
                    .as_bytes()
                    .chunks(4)
                    .map(|u| u16::from_str_radix(std::str::from_utf8(u).unwrap(), 16).unwrap())
                    .collect();
                chars.extend(String::from_utf16(&units).unwrap().chars());
            }
        }
    }
    fonts
        .into_iter()
        .map(|(name, mut chars)| {
            chars.sort_unstable();
            chars.dedup();
            (name, chars.into_iter().collect())
        })
        .collect()
}

fn drawn(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, chars)| {
            let mut chars: Vec<char> = chars.chars().collect();
            chars.sort_unstable();
            chars.dedup();
            (name.to_string(), chars.into_iter().collect())
        })
        .collect()
}

#[test]
fn each_subset_face_draws_only_the_characters_of_its_range() {
    // As Google Fonts splits a family: one face for the capitals, one for the rest, both of
    // the same family, weight and style. Inter's own character map also covers the digits,
    // the space and x and q; Lato's covers the capitals.
    let faces = "@font-face { font-family: Split; src: url(inter.ttf); unicode-range: U+41-5A }
        @font-face { font-family: Split; src: url(lato.ttf); unicode-range: U+0-40, U+5B-FF }";
    for reversed in [false, true] {
        let out = print(faces, "Split", "DUN OPEN 1,234.56 xqx", reversed);
        assert_eq!(
            drawn_by(&out.bytes),
            drawn(&[("Inter24pt-Regular", "DUNOPE"), ("Lato-Regular", " 1,23456.xq")]),
            "loaded last rule first: {reversed}"
        );
    }
}

#[test]
fn overlapping_faces_are_checked_last_rule_first() {
    // A face without a range covers every character: the later rule decides.
    let lato = "@font-face { font-family: Over; src: url(lato.ttf) }";
    let inter = "@font-face { font-family: Over; src: url(inter.ttf); unicode-range: U+41-5A }";
    for reversed in [false, true] {
        let out = print(&format!("{lato} {inter}"), "Over", "DUN xqx", reversed);
        assert_eq!(
            drawn_by(&out.bytes),
            drawn(&[("Inter24pt-Regular", "DUN"), ("Lato-Regular", " xq")]),
            "capitals' face last, loaded last rule first: {reversed}"
        );
        let out = print(&format!("{inter} {lato}"), "Over", "DUN xqx", reversed);
        assert_eq!(
            drawn_by(&out.bytes),
            drawn(&[("Lato-Regular", "DUN xq")]),
            "capitals' face first, loaded last rule first: {reversed}"
        );
    }
}
