use console::style;

pub fn header(text: impl AsRef<str>) -> String {
    style(text.as_ref()).cyan().to_string()
}

pub fn success(text: impl AsRef<str>) -> String {
    style(text.as_ref()).green().to_string()
}

pub fn error(text: impl AsRef<str>) -> String {
    style(text.as_ref()).red().to_string()
}

pub fn warning(text: impl AsRef<str>) -> String {
    style(text.as_ref()).yellow().to_string()
}

pub fn emotion(text: impl AsRef<str>) -> String {
    style(text.as_ref()).magenta().to_string()
}

pub fn dim(text: impl AsRef<str>) -> String {
    style(text.as_ref()).dim().to_string()
}
