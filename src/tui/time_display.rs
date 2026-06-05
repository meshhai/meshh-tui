use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

pub(super) fn format_delivery_timestamp(timestamp: Option<&str>) -> String {
    let Some(timestamp) = timestamp else {
        return "-".to_owned();
    };

    let local_offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    match compact_timestamp_at_offset(timestamp, local_offset) {
        Some(compact) => compact,
        None => timestamp.to_owned(),
    }
}

pub(super) fn compact_timestamp_at_offset(timestamp: &str, offset: UtcOffset) -> Option<String> {
    if let Ok(parsed) = OffsetDateTime::parse(timestamp, &Rfc3339) {
        return Some(format_compact_datetime(parsed.to_offset(offset)));
    }

    compact_iso_timestamp(timestamp)
}

fn format_compact_datetime(datetime: OffsetDateTime) -> String {
    let month = match datetime.month() {
        time::Month::January => "Jan",
        time::Month::February => "Feb",
        time::Month::March => "Mar",
        time::Month::April => "Apr",
        time::Month::May => "May",
        time::Month::June => "Jun",
        time::Month::July => "Jul",
        time::Month::August => "Aug",
        time::Month::September => "Sep",
        time::Month::October => "Oct",
        time::Month::November => "Nov",
        time::Month::December => "Dec",
    };

    format!(
        "{month} {:02} {:02}:{:02}",
        datetime.day(),
        datetime.hour(),
        datetime.minute()
    )
}

fn compact_iso_timestamp(timestamp: &str) -> Option<String> {
    let parts = timestamp
        .split_once('T')
        .or_else(|| timestamp.split_once(' '))?;
    if parts.0.len() != 10 || parts.1.len() < 5 || !parts.0.is_ascii() || !parts.1.is_ascii() {
        return None;
    }

    let month = match &parts.0[5..7] {
        "01" => "Jan",
        "02" => "Feb",
        "03" => "Mar",
        "04" => "Apr",
        "05" => "May",
        "06" => "Jun",
        "07" => "Jul",
        "08" => "Aug",
        "09" => "Sep",
        "10" => "Oct",
        "11" => "Nov",
        "12" => "Dec",
        _ => return None,
    };
    let day = &parts.0[8..10];
    let time = &parts.1[..5];

    if !day.bytes().all(|byte| byte.is_ascii_digit())
        || !time.as_bytes()[0].is_ascii_digit()
        || !time.as_bytes()[1].is_ascii_digit()
        || time.as_bytes()[2] != b':'
        || !time.as_bytes()[3].is_ascii_digit()
        || !time.as_bytes()[4].is_ascii_digit()
    {
        return None;
    }

    Some(format!("{month} {day} {time}"))
}
