//! QR code (SVG) for pairing links.

/// `devbridge://pair?h=<host>&p=<port>&c=<code>` as an SVG string.
pub fn pairing_svg(host: &str, port: u16, code: &str) -> Option<String> {
    let url = format!("devbridge://pair?h={host}&p={port}&c={code}");
    let qr = qrcode::QrCode::new(url.as_bytes()).ok()?;
    Some(qr.render::<qrcode::render::svg::Color<'_>>().min_dimensions(180, 180).quiet_zone(true).build())
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders_svg() {
        let s = super::pairing_svg("192.168.1.20", 47821, "7KQ2-M9XD").unwrap();
        assert!(s.starts_with("<?xml") || s.starts_with("<svg"));
    }
}
