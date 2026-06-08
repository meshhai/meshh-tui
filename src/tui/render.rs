use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap},
};

use crate::api::DeliveryDetail;

use super::{
    AppState, DeliveryListStatus, DetailStatus, Screen, StreamStatus,
    time_display::format_delivery_timestamp,
};

/// Renders the TUI from immutable app state.
pub fn render(frame: &mut Frame<'_>, state: &AppState) {
    let show_update_notice = state.update_notice().is_some();
    let constraints = if show_update_notice {
        vec![
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(2),
        ]
    } else {
        vec![
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(2),
        ]
    };
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(frame.area());

    render_header(frame, layout[0], state);

    let content_area_index = if show_update_notice {
        render_update_notice(frame, layout[1], state);
        2
    } else {
        1
    };
    let footer_area_index = content_area_index + 1;

    match state.screen() {
        Screen::DeliveryList => render_list(frame, layout[content_area_index], state),
        Screen::DeliveryDetail { .. } => render_detail(frame, layout[content_area_index], state),
    }

    render_footer(frame, layout[footer_area_index], state);
}

fn render_header(frame: &mut Frame<'_>, area: ratatui::layout::Rect, state: &AppState) {
    let activity = header_activity(state);
    let row_count = state.deliveries().len();
    let rows = match row_count {
        1 => "1 row".to_owned(),
        count => format!("{count} rows"),
    };
    let selected = match (state.selected_index(), row_count) {
        (Some(index), count) if count > 0 => format!("row {}/{}", index + 1, count),
        _other => "no row".to_owned(),
    };
    let line = Line::from(vec![
        Span::styled(
            format!("meshh-tui v{}", env!("CARGO_PKG_VERSION")),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | "),
        Span::raw(activity),
        Span::raw(" | "),
        Span::raw(rows),
        Span::raw(" | "),
        Span::raw(selected),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

fn render_update_notice(frame: &mut Frame<'_>, area: ratatui::layout::Rect, state: &AppState) {
    let Some(notice) = state.update_notice() else {
        return;
    };
    let line = Line::from(Span::styled(
        notice.message(),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    ));

    frame.render_widget(Paragraph::new(line), area);
}

fn header_activity(state: &AppState) -> String {
    match state.delivery_list_status() {
        DeliveryListStatus::Loading => "loading".to_owned(),
        DeliveryListStatus::Error(error) => error.heading().to_owned(),
        DeliveryListStatus::Empty | DeliveryListStatus::Ready => {
            stream_activity(state.stream_status())
        }
    }
}

fn stream_activity(stream_status: &StreamStatus) -> String {
    match stream_status {
        StreamStatus::Disconnected => "offline".to_owned(),
        StreamStatus::Connecting => "connecting".to_owned(),
        StreamStatus::Live => "live".to_owned(),
        StreamStatus::Reconnecting(error) => format!("reconnecting: {}", error.heading()),
        StreamStatus::Error(error) => error.heading().to_owned(),
    }
}

fn render_list(frame: &mut Frame<'_>, area: ratatui::layout::Rect, state: &AppState) {
    if state.deliveries().is_empty() {
        let message = empty_list_message(state);

        frame.render_widget(
            Paragraph::new(message)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::DarkGray)),
                )
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let rows = state.deliveries().iter().map(|item| {
        Row::new(vec![
            Cell::from(format_delivery_timestamp(item.display_timestamp())),
            Cell::from(item.source_context().unwrap_or("-").to_owned()),
            Cell::from(item.headline().to_owned()),
            Cell::from(item.status().as_str().to_owned()),
        ])
        .style(Style::default().fg(Color::White))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(18),
            Constraint::Min(24),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new(vec!["Published", "Source", "Headline", "Status"]).style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    )
    .column_spacing(1)
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("> ");
    let mut table_state = TableState::default().with_selected(state.selected_index());

    frame.render_stateful_widget(table, area, &mut table_state);
}

fn empty_list_message(state: &AppState) -> &str {
    match state.delivery_list_status() {
        DeliveryListStatus::Loading => "Loading route deliveries...",
        DeliveryListStatus::Error(error) => error.message(),
        DeliveryListStatus::Empty | DeliveryListStatus::Ready => match state.stream_status() {
            StreamStatus::Reconnecting(error) | StreamStatus::Error(error) => error.message(),
            _other => "No route deliveries yet.",
        },
    }
}

fn render_detail(frame: &mut Frame<'_>, area: ratatui::layout::Rect, state: &AppState) {
    let text = match state.detail_status() {
        DetailStatus::Hidden => Text::from("No delivery selected."),
        DetailStatus::Loading => Text::from("Loading delivery detail..."),
        DetailStatus::Error(error) => Text::from(vec![
            Line::from(error.heading()),
            Line::from(""),
            Line::from(error.message().to_owned()),
        ]),
        DetailStatus::Ready => detail_text(state.detail()),
    };

    frame.render_widget(
        Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn detail_text(detail: Option<&DeliveryDetail>) -> Text<'static> {
    let Some(detail) = detail else {
        return Text::from("No delivery selected.");
    };
    let routes = if detail.matched_routes().is_empty() {
        "-".to_owned()
    } else {
        detail
            .matched_routes()
            .iter()
            .map(|route| route.name())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let summary_or_body = detail
        .summary()
        .or_else(|| detail.body())
        .unwrap_or("No summary provided.");

    Text::from(vec![
        Line::from(Span::styled(
            detail.headline().to_owned(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("Status: {}", detail.status().as_str())),
        Line::from(format!(
            "Published: {}",
            format_delivery_timestamp(detail.display_timestamp())
        )),
        Line::from(format!(
            "Source: {}",
            detail.source_context().unwrap_or("-")
        )),
        Line::from(format!(
            "Source URL: {}",
            detail.source_url().unwrap_or("-")
        )),
        Line::from(format!("Matched routes: {routes}")),
        Line::from(""),
        Line::from(summary_or_body.to_owned()),
    ])
}

fn render_footer(frame: &mut Frame<'_>, area: ratatui::layout::Rect, state: &AppState) {
    let message = match state.screen() {
        Screen::DeliveryList => "up/down select | enter open | r refresh | q quit",
        Screen::DeliveryDetail { .. } => "b back | r reload detail | q quit",
    };

    frame.render_widget(Paragraph::new(message), area);
}
