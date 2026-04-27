// QR panel: renders the scannable pairing QR code and the plain-text
// App URL + OTK sidecar below it.

use qrcode::{QrCode, render::unicode};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use super::{State, now_secs};

pub(super) fn render_qr_panel(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    // Determine OTK expiry for dim styling.
    let otk_expired = match state.otk_expires_at {
        Some(exp) => now_secs() >= exp,
        None => false,
    };

    let title = if otk_expired {
        " Pair a device (key expired — press r) "
    } else {
        " Pair a device "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if state.tunnel_url.is_none() {
        // Tunnel not yet ready — show a placeholder instead of passing the
        // fallback string into QrCode::new(), which would render garbage.
        let placeholder = Paragraph::new("Tunnel not ready yet\u{2026}")
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(placeholder, inner);
        return;
    }

    // Reserve 3 rows at the bottom for the App URL + Key sidecar so users
    // without a QR scanner can still read the URL/token directly. QR sits
    // above; sidecar below. 3 rows = blank separator + URL + Key lines.
    let sidecar_h: u16 = 3;
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(sidecar_h)])
        .split(inner);
    let qr_area = layout[0];
    let sidecar_area = layout[1];

    let payload = state.qr_payload();
    let body = match QrCode::new(payload.as_bytes()) {
        Ok(code) => code
            .render::<unicode::Dense1x2>()
            .dark_color(unicode::Dense1x2::Light)
            .light_color(unicode::Dense1x2::Dark)
            .quiet_zone(false)
            .build(),
        Err(_) => payload,
    };

    // Dim the QR when the OTK has expired — scanning it will fail anyway.
    let qr_style = if otk_expired {
        Style::default().add_modifier(Modifier::DIM)
    } else {
        Style::default()
    };
    let para = Paragraph::new(body).alignment(Alignment::Center).style(qr_style);
    f.render_widget(para, qr_area);

    render_qr_sidecar(f, sidecar_area, state, otk_expired);
}

/// Plain-text App URL + Key + expiry under the QR. Lets users without a
/// scanner read the values directly (matches the onboarding pane).
pub(super) fn render_qr_sidecar(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &State,
    otk_expired: bool,
) {
    let url = state.tunnel_url.as_deref().unwrap_or("—");
    let key_line = match (&state.otk_token, state.otk_expires_at) {
        (Some(token), Some(expires_at)) => {
            let remaining = expires_at - now_secs();
            if remaining <= 0 {
                format!("Key: {token}  (expired)")
            } else {
                let mins = remaining / 60;
                let secs = remaining % 60;
                format!("Key: {token}  (expires {mins:02}:{secs:02})")
            }
        }
        _ => "Key: — (press r to generate)".into(),
    };
    let key_style = if otk_expired || state.otk_token.is_none() {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
            .fg(super::super::step_progress::BRAND)
            .add_modifier(Modifier::BOLD)
    };
    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("App URL: ", Style::default().fg(Color::DarkGray)),
            Span::styled(url, Style::default().fg(super::super::step_progress::BRAND)),
        ]),
        Line::from(Span::styled(key_line, key_style)),
    ];
    f.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center).wrap(Wrap { trim: true }),
        area,
    );
}
