//! Parse one `key=value` configuration line with escaped separators.

pub fn parse_line(line: &str) -> Result<Option<(String, String)>, &'static str> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') { return Ok(None); }
    let mut split_at = None;
    let mut escaped = false;
    for (index, character) in trimmed.char_indices() {
        if escaped { escaped = false; continue; }
        if character == '\\' { escaped = true; continue; }
        if character == '=' { split_at = Some(index); break; }
    }
    let index = split_at.ok_or("configuration line must contain '='")?;
    let key = trimmed[..index].trim();
    let value = trimmed[index + 1..].trim();
    if key.is_empty() { return Err("configuration key cannot be empty"); }
    Ok(Some((key.to_owned(), unescape(value)?)))
}

fn unescape(value: &str) -> Result<String, &'static str> {
    let mut result = String::with_capacity(value.len());
    let mut escaped = false;
    for character in value.chars() {
        if escaped { result.push(character); escaped = false; }
        else if character == '\\' { escaped = true; }
        else { result.push(character); }
    }
    if escaped { Err("value ends with an escape") } else { Ok(result) }
}
