//! Decoders for sensor curves and sensor XML param values.
//!
//! Krita stores curve transfer functions as a tiny ad-hoc string format —
//! `x1,y1;x2,y2;…` — and sensor configurations as a small XML blob like
//! `<params id="pressure"/>`. These show up as the inner text of `<param>`
//! elements with `type="string"`.

/// Try parsing `s` as a Krita curve point string (`x1,y1;x2,y2;…`).
///
/// The grammar is permissive — Krita accepts trailing semicolons and runs of
/// whitespace. Returns `None` on any malformed input so the caller can fall
/// back to other decoders.
pub fn parse_curve_points(s: &str) -> Option<Vec<(f32, f32)>> {
    let s = s.trim();
    if s.is_empty() || !s.contains(',') {
        return None;
    }
    let mut points = Vec::new();
    for chunk in s.split(';') {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        let (x, y) = chunk.split_once(',')?;
        let x = x.trim().parse::<f32>().ok()?;
        let y = y.trim().parse::<f32>().ok()?;
        points.push((x, y));
    }
    if points.is_empty() {
        None
    } else {
        Some(points)
    }
}

/// If `s` is a Krita sensor XML blob, return its `id` attribute (e.g.
/// `pressure`, `tilt`, `speed`). The sensor blob looks like
/// `<!DOCTYPE params> <params id="pressure"/>` — sometimes wrapped, sometimes
/// without the doctype. We do a best-effort attribute extract rather than a
/// full XML parse.
pub fn extract_sensor_id(s: &str) -> Option<String> {
    let lower = s.to_ascii_lowercase();
    if !lower.contains("<params") {
        return None;
    }
    let after = s.split("<params").nth(1)?;
    let id_pos = after.find("id=")?;
    let after_eq = &after[id_pos + 3..];
    let quote = after_eq.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let body = &after_eq[1..];
    let end = body.find(quote)?;
    Some(body[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_identity_curve() {
        assert_eq!(
            parse_curve_points("0,0;1,1;"),
            Some(vec![(0.0, 0.0), (1.0, 1.0)])
        );
    }

    #[test]
    fn parses_three_point_curve() {
        assert_eq!(
            parse_curve_points("0,0;0.5,0.8;1,1"),
            Some(vec![(0.0, 0.0), (0.5, 0.8), (1.0, 1.0)])
        );
    }

    #[test]
    fn rejects_non_curve_strings() {
        assert_eq!(parse_curve_points(""), None);
        assert_eq!(parse_curve_points("plain"), None);
        assert_eq!(parse_curve_points("not,really;a;curve"), None);
    }

    #[test]
    fn extracts_pressure_sensor_id() {
        let xml = r#"<!DOCTYPE params> <params id="pressure"/> "#;
        assert_eq!(extract_sensor_id(xml).as_deref(), Some("pressure"));
    }

    #[test]
    fn extracts_tilt_sensor_id_no_doctype() {
        let xml = r#"<params id="tilt_x"/>"#;
        assert_eq!(extract_sensor_id(xml).as_deref(), Some("tilt_x"));
    }

    #[test]
    fn returns_none_for_non_sensor_xml() {
        assert_eq!(
            extract_sensor_id("<color><RGB r='1' g='0' b='0'/></color>"),
            None
        );
    }
}
