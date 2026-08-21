// SPDX-License-Identifier: GPL-3.0-only
//! Code 128 (subset B) encoding.
//!
//! Cheap wireless barcode scanners are usually 1D lasers that cannot read a QR
//! code at all, so labels carry a linear barcode alongside the QR. Code 128B
//! covers the full printable ASCII range, which is everything a label code or
//! a retail product number needs.

/// Element widths for each symbol value, bar first, alternating bar/space.
/// Every pattern is six elements totalling eleven modules.
const PATTERNS: [&str; 107] = [
    "212222", "222122", "222221", "121223", "121322", "131222", "122213", "122312", "132212",
    "221213", "221312", "231212", "112232", "122132", "122231", "113222", "123122", "123221",
    "223211", "221132", "221231", "213212", "223112", "312131", "311222", "321122", "321221",
    "312212", "322112", "322211", "212123", "212321", "232121", "111323", "131123", "131321",
    "112313", "132113", "132311", "211313", "231113", "231311", "112133", "112331", "132131",
    "113123", "113321", "133121", "313121", "211331", "231131", "213113", "213311", "213131",
    "311123", "311321", "331121", "312113", "312311", "332111", "314111", "221411", "431111",
    "111224", "111422", "121124", "121421", "141122", "141221", "112214", "112412", "122114",
    "122411", "142112", "142211", "241211", "221114", "413111", "241112", "134111", "111242",
    "121142", "121241", "114212", "124112", "124211", "411212", "421112", "421211", "212141",
    "214121", "412121", "111143", "111341", "131141", "114113", "114311", "411113", "411311",
    "113141", "114131", "311141", "411131", "211412", "211214", "211232", "233111",
];

const START_B: usize = 104;
const STOP: &str = "2331112";
/// Blank margin either side, in modules. Scanners need it to find the symbol.
const QUIET_ZONE: u32 = 10;

/// Turns text into alternating bar/space widths, starting with a bar.
pub fn code128_widths(data: &str) -> Result<Vec<u32>, String> {
    if data.is_empty() {
        return Err("nothing to encode".to_string());
    }
    let values: Vec<usize> = data
        .chars()
        .map(|c| {
            let code = c as u32;
            if (32..127).contains(&code) {
                Ok(code as usize - 32)
            } else {
                Err(format!("'{c}' cannot be encoded as Code 128"))
            }
        })
        .collect::<Result<_, _>>()?;

    // Checksum: start value plus each symbol weighted by its position.
    let checksum = values
        .iter()
        .enumerate()
        .fold(START_B, |acc, (i, v)| acc + (i + 1) * v)
        % 103;

    let mut symbols = vec![START_B];
    symbols.extend(&values);
    symbols.push(checksum);

    let mut widths: Vec<u32> = Vec::new();
    for symbol in symbols {
        for ch in PATTERNS[symbol].chars() {
            widths.push(ch.to_digit(10).expect("pattern digits are 1-4"));
        }
    }
    for ch in STOP.chars() {
        widths.push(ch.to_digit(10).expect("pattern digits are 1-4"));
    }
    Ok(widths)
}

/// Total width of the symbol in modules, quiet zones included.
pub fn code128_modules(data: &str) -> Result<u32, String> {
    Ok(code128_widths(data)?.iter().sum::<u32>() + QUIET_ZONE * 2)
}

/// Renders the barcode as an SVG whose viewBox is measured in modules, so the
/// caller sizes it in millimetres with CSS and the bars scale exactly.
pub fn code128_svg(data: &str, height_modules: u32) -> Result<String, String> {
    let widths = code128_widths(data)?;
    let total = code128_modules(data)?;
    let mut rects = String::new();
    let mut x = QUIET_ZONE;
    for (index, width) in widths.iter().enumerate() {
        if index % 2 == 0 {
            // Even elements are bars; odd are the spaces between them.
            rects.push_str(&format!(
                r#"<rect x="{x}" y="0" width="{width}" height="{height_modules}"/>"#
            ));
        }
        x += width;
    }
    Ok(format!(
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {} {}" "#,
            r#"preserveAspectRatio="none" shape-rendering="crispEdges" fill="black">"#,
            r#"<rect x="0" y="0" width="{}" height="{}" fill="white"/>{}</svg>"#
        ),
        total, height_modules, total, height_modules, rects
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_is_eleven_modules_wide() {
        // A mistyped digit anywhere in the table would break this.
        for (value, pattern) in PATTERNS.iter().enumerate() {
            let sum: u32 = pattern.chars().map(|c| c.to_digit(10).unwrap()).sum();
            assert_eq!(sum, 11, "pattern {value} ({pattern}) is not 11 modules");
            assert_eq!(
                pattern.len(),
                6,
                "pattern {value} does not have six elements"
            );
        }
    }

    #[test]
    fn every_pattern_has_even_bar_parity() {
        // Code 128 symbol characters all have an even total bar width; scanners
        // rely on it, and it catches table typos the width check would miss.
        for (value, pattern) in PATTERNS.iter().enumerate() {
            let bars: u32 = pattern
                .chars()
                .step_by(2)
                .map(|c| c.to_digit(10).unwrap())
                .sum();
            assert_eq!(
                bars % 2,
                0,
                "pattern {value} ({pattern}) has odd bar parity"
            );
        }
    }

    /// Reads bar/space widths back into text the way a scanner would.
    fn decode(widths: &[u32]) -> Result<String, String> {
        // The trailing stop pattern is seven elements, not six; drop it first
        // so the rest divides evenly into symbols.
        let stop: String = widths[widths.len() - 7..]
            .iter()
            .map(|w| w.to_string())
            .collect();
        if stop != STOP {
            return Err(format!("symbol does not end with a stop pattern: {stop}"));
        }
        let symbols: Vec<String> = widths[..widths.len() - 7]
            .chunks(6)
            .map(|c| c.iter().map(|w| w.to_string()).collect())
            .collect();
        let mut values = Vec::new();
        for symbol in &symbols {
            let value = PATTERNS
                .iter()
                .position(|p| p == symbol)
                .ok_or_else(|| format!("unknown pattern {symbol}"))?;
            values.push(value);
        }
        let start = values.remove(0);
        assert_eq!(start, START_B, "expected a Code B start symbol");
        let check = values.pop().ok_or("missing checksum")?;
        let expected = values
            .iter()
            .enumerate()
            .fold(START_B, |acc, (i, v)| acc + (i + 1) * v)
            % 103;
        if check != expected {
            return Err(format!("checksum {check} should have been {expected}"));
        }
        Ok(values.iter().map(|v| char::from(*v as u8 + 32)).collect())
    }

    #[test]
    fn encodes_and_decodes_a_label_code() {
        let widths = code128_widths("BX-7K3Q").unwrap();
        assert_eq!(decode(&widths).unwrap(), "BX-7K3Q");
    }

    #[test]
    fn encodes_and_decodes_a_product_number() {
        let widths = code128_widths("012345678905").unwrap();
        assert_eq!(decode(&widths).unwrap(), "012345678905");
    }

    #[test]
    fn symbol_layout_matches_the_specification() {
        // start + data + checksum + stop, with the stop's extra termination bar.
        let widths = code128_widths("AB").unwrap();
        assert_eq!(widths.len(), 6 * 4 + 7);
        let modules: u32 = widths.iter().sum();
        assert_eq!(modules, 11 * 4 + 13);
        assert_eq!(code128_modules("AB").unwrap(), modules + 20);
    }

    #[test]
    fn rejects_characters_outside_the_printable_range() {
        assert!(code128_widths("caffè").is_err());
        assert!(code128_widths("").is_err());
    }

    #[test]
    fn svg_starts_and_ends_with_a_bar_inside_the_quiet_zone() {
        let svg = code128_svg("BX-7K3Q", 30).unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("viewBox=\"0 0 "));
        assert!(
            svg.contains("<rect x=\"10\""),
            "first bar sits after the quiet zone"
        );
    }
}
