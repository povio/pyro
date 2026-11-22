//! HTML window.open feature parsing and CSSOM View geometry requests.

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowFeatures {
    pub noopener: bool,
    pub noreferrer: bool,
    pub is_popup: bool,
    resizing_disabled: bool,
    /// CDP distinguishes an explicitly requested zero size from an omitted feature.
    width_specified: bool,
    height_specified: bool,
    /// Requested viewport dimensions. An absent, invalid or zero size leaves it unchanged.
    pub width: Option<i32>,
    pub height: Option<i32>,
    /// Requested screen coordinates. An explicit invalid coordinate denotes zero.
    pub left: Option<i32>,
    pub top: Option<i32>,
}

impl WindowFeatures {
    /// CDP Page.windowOpen describes the parsed features of the new window.
    pub fn enabled_features(&self) -> Vec<String> {
        let mut names = Vec::new();
        let geometry = [
            ("left", self.left),
            ("top", self.top),
            ("width", self.width_specified.then_some(self.width.unwrap_or(0))),
            ("height", self.height_specified.then_some(self.height.unwrap_or(0))),
        ];
        for (name, value) in geometry {
            if let Some(value) = value {
                names.push(format!("{name}={value}"));
            }
        }
        if !self.is_popup {
            names.extend(["menubar", "toolbar", "status", "scrollbars"].map(str::to_owned));
        }
        if !self.resizing_disabled {
            names.push("resizable".into());
        }
        if self.noopener {
            names.push("noopener".into());
        }
        names
    }

    pub fn parse(input: &str) -> Self {
        let input = input.to_ascii_lowercase();
        let mut features = tokenize(&input);
        let noreferrer = features.remove("noreferrer").is_some_and(boolean);
        let noopener = features.remove("noopener").is_some_and(boolean) || noreferrer;

        let is_set = |name, default| features.get(name).map_or(default, |value| boolean(value));
        let is_popup = if let Some(value) = features.get("popup") {
            boolean(value)
        } else {
            !features.is_empty()
                && ((!is_set("location", false) && !is_set("toolbar", false))
                    || !is_set("menubar", false)
                    || !is_set("resizable", true)
                    || !is_set("scrollbars", false)
                    || !is_set("status", false))
        };
        let geometry = |name| features.get(name).map(|value| integer(value));
        Self {
            noopener,
            noreferrer,
            is_popup,
            resizing_disabled: !is_set("resizable", true),
            width_specified: features.contains_key("width"),
            height_specified: features.contains_key("height"),
            width: geometry("width").filter(|&n| n != 0),
            height: geometry("height").filter(|&n| n != 0),
            left: geometry("left"),
            top: geometry("top"),
        }
    }
}

fn separator(c: char) -> bool {
    c.is_ascii_whitespace() || matches!(c, ',' | '=')
}

// ASCII whitespace can separate two names, or surround the equals sign.
// A comma ends the current feature even when it follows an equals sign.
fn tokenize(mut rest: &str) -> HashMap<&str, &str> {
    let mut features = HashMap::new();
    loop {
        rest = rest.trim_start_matches(separator);
        if rest.is_empty() {
            return features;
        }
        let (name, after) = rest.split_at(rest.find(separator).unwrap_or(rest.len()));
        rest = after.trim_start_matches(|c: char| c.is_ascii_whitespace());
        let mut value = "";
        if rest.starts_with('=') {
            rest = rest.trim_start_matches(|c: char| c == '=' || c.is_ascii_whitespace());
            (value, rest) = rest.split_at(rest.find(separator).unwrap_or(rest.len()));
        }
        let name = match name {
            "screenx" => "left",
            "screeny" => "top",
            "innerwidth" => "width",
            "innerheight" => "height",
            _ => name,
        };
        features.insert(name, value);
    }
}

fn boolean(value: &str) -> bool {
    matches!(value, "" | "yes" | "true") || integer(value) != 0
}

// Feature tokenization already skipped leading ASCII whitespace. HTML collects
// the signed decimal prefix and treats a missing digit as an error. Both
// feature algorithms substitute zero on error. Saturate to our coordinate
// range without wrapping a nonzero integer into a false boolean.
fn integer(value: &str) -> i32 {
    let (negative, digits) = match value.as_bytes().split_first() {
        Some((b'-', digits)) => (true, digits),
        Some((b'+', digits)) => (false, digits),
        _ => (false, value.as_bytes()),
    };
    let mut result: i32 = 0;
    for &digit in digits {
        if !digit.is_ascii_digit() {
            break;
        }
        let n = i32::from(digit - b'0');
        result = result.saturating_mul(10).saturating_add(if negative { -n } else { n });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separators_aliases_and_last_value_win() {
        let f = WindowFeatures::parse(
            " ,== WIDTH=22 InnerWidth == +405.5, HEIGHT=404 innerheight=-21 screenX =9 LEFT=7 screenY=oops",
        );
        assert_eq!((f.width, f.height, f.left, f.top), (Some(405), Some(-21), Some(7), Some(0)));
        assert_eq!(WindowFeatures::parse("width=,height=0").width, None);
        assert!(WindowFeatures::parse("popup\u{000b}=yes").is_popup); // unknown name requests a popup
        assert!(!WindowFeatures::parse("popup=\u{000b}yes").is_popup);
    }

    #[test]
    fn protocol_reports_enabled_chrome_and_explicit_geometry() {
        assert_eq!(WindowFeatures::parse("width=0,height=0").enabled_features(), ["width=0", "height=0", "resizable"]);
        assert_eq!(WindowFeatures::parse("innerWidth=-404,innerHeight=405.5,left=12").enabled_features(),
            ["left=12", "width=-404", "height=405", "resizable"]);
        assert_eq!(WindowFeatures::parse("noreferrer").enabled_features(),
            ["menubar", "toolbar", "status", "scrollbars", "resizable", "noopener"]);
    }

    #[test]
    fn boolean_values_and_popup_controls() {
        for value in ["", "YES", "true", "1foo", "-1", "+123", "99999999999999999999"] {
            assert!(WindowFeatures::parse(&format!("popup={value}")).is_popup, "{value}");
        }
        for value in ["0", "00foo", "'1'", "no", "false", "٠١"] {
            assert!(!WindowFeatures::parse(&format!("popup={value}")).is_popup, "{value}");
        }
        assert_eq!(WindowFeatures::parse(" == , "), WindowFeatures::default());
        assert_eq!(WindowFeatures::parse("noreferrer=0,noopener=no"), WindowFeatures::default());
        let n = WindowFeatures::parse("noopener=0,noreferrer");
        assert!(n.noreferrer && n.noopener && !n.is_popup);
        assert!(WindowFeatures::parse("unrecognized").is_popup);
        assert!(!WindowFeatures::parse("unrecognized,popup=0").is_popup);
        assert!(!WindowFeatures::parse("toolbar,menubar,scrollbars,status").is_popup);
        assert!(WindowFeatures::parse("location,menubar,scrollbars,status,resizable=0").is_popup);
    }

    #[test]
    fn saturates_coordinates_and_preserves_omission() {
        let f = WindowFeatures::parse("left=-99999999999,top=99999999999,height=not-an-int");
        assert_eq!((f.left, f.top, f.width, f.height), (Some(i32::MIN), Some(i32::MAX), None, None));
        assert_eq!(WindowFeatures::parse("left=-2147483648").left, Some(i32::MIN));
    }
}
