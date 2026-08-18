//! JSON Schema → egui form renderer.
//!
//! Renders an object schema into typed controls (`string` / `number` /
//! `integer` / `boolean` / `object` / `array` / nullable / `enum` /
//! `oneOf`), honors `x-ene-ui` field metadata (group / order / control /
//! advanced / impact / `options_path`), preserves unknown keys, and falls
//! back to a raw JSON editor for anything the typed path cannot express.
//!
//! The renderer edits a `serde_json::Value` in place and returns whether it
//! changed; it never constructs a new value from scratch, so keys outside
//! the schema survive every edit. Secret fields render masked and never
//! echo the stored value into a text buffer.

use super::draft::FieldImpact;
use std::collections::BTreeMap;

pub(crate) const BUILTIN_PROVIDER_GROUP_IDS: &[&str] =
    &["connection", "engine", "model", "runtime", "voice"];

#[derive(Debug, Clone, Default)]
pub struct UiMetadata {
    pub group: Option<String>,
    pub label_key: Option<String>,
    pub description_key: Option<String>,
    pub order: Option<f64>,
    pub control: Option<String>,
    pub advanced: bool,
    pub impact: Option<FieldImpact>,
    pub options_path: Option<String>,
    pub secret: bool,
    /// `x-ene-ui.slider: {min, max, step}` for number fields.
    pub slider: Option<(f64, f64, f64)>,
}

impl UiMetadata {
    #[must_use]
    pub fn from_schema(schema: &serde_json::Value) -> Self {
        let mut meta = schema
            .get("x-ene-ui")
            .and_then(serde_json::Value::as_object)
            .map_or_else(Self::default, |object| Self {
                group: object
                    .get("group")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                label_key: object
                    .get("label_key")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                description_key: object
                    .get("description_key")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                order: object.get("order").and_then(serde_json::Value::as_f64),
                control: object
                    .get("control")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                advanced: object
                    .get("advanced")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                impact: object
                    .get("impact")
                    .and_then(serde_json::Value::as_str)
                    .and_then(FieldImpact::parse),
                options_path: object
                    .get("options_path")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                secret: false,
                slider: object
                    .get("slider")
                    .and_then(serde_json::Value::as_object)
                    .and_then(|slider| {
                        Some((
                            slider.get("min")?.as_f64()?,
                            slider.get("max")?.as_f64()?,
                            slider
                                .get("step")
                                .and_then(serde_json::Value::as_f64)
                                .unwrap_or(0.1),
                        ))
                    }),
            });
        if meta.secret {
            return meta;
        }
        meta.secret = schema
            .get("x-ene-secret")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
            || meta.control.as_deref() == Some("secret");
        meta
    }

    #[must_use]
    pub const fn is_advanced(&self) -> bool {
        self.advanced
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SchemaFormOptions<'a> {
    /// Set by search results to reveal advanced (`x-ene-ui.advanced`) fields.
    pub show_advanced: bool,
    pub show_impact: bool,
    /// Draft epoch that scopes transient edit buffers (e.g. secret
    /// replacement text). Bump it on every apply so a committed secret is
    /// never displayed again from a stale buffer.
    pub epoch: u64,
    /// Dynamic options per dotted field path (from the plugin's
    /// `ListConfigOptions`), used to render combos for fields whose
    /// `x-ene-ui.options_path` matches. `None` renders plain text fields.
    pub options: Option<&'a BTreeMap<String, Vec<(String, String)>>>,
}

#[must_use]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "used by schema-form tests; production pages edit opaque core JSON"
    )
)]
pub fn profiles_schema(schema: &serde_json::Value) -> Option<&serde_json::Value> {
    schema
        .get("x-ene-profiles-schema")
        .or_else(|| schema.get("properties")?.get("profiles"))
}

/// Returns `true` when any control changed the value. `path` is the dotted
/// config path used for stable egui ids and issue display.
pub fn schema_object_form(
    ui: &mut egui::Ui,
    schema: &serde_json::Value,
    value: &mut serde_json::Value,
    path: &str,
    options: SchemaFormOptions<'_>,
) -> bool {
    if !value.is_object() {
        return raw_json_form(ui, value, path, schema, options.epoch);
    }
    let Some(properties) = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
    else {
        return raw_json_form(ui, value, path, schema, options.epoch);
    };

    let mut entries: Vec<(&String, &serde_json::Value)> = properties.iter().collect();
    entries.sort_by(|(left_name, left_schema), (right_name, right_schema)| {
        let left_order = UiMetadata::from_schema(left_schema).order;
        let right_order = UiMetadata::from_schema(right_schema).order;
        left_order
            .partial_cmp(&right_order)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left_name.cmp(right_name))
    });

    let mut changed = false;
    let mut grouped: BTreeMap<String, Vec<(&String, &serde_json::Value)>> = BTreeMap::new();
    let mut ungrouped = Vec::new();
    for (name, property_schema) in entries {
        let meta = UiMetadata::from_schema(property_schema);
        if meta.is_advanced() && !options.show_advanced {
            continue;
        }
        match meta.group {
            Some(group) => grouped
                .entry(group)
                .or_default()
                .push((name, property_schema)),
            None => ungrouped.push((name, property_schema)),
        }
    }

    for (name, property_schema) in ungrouped {
        changed |= render_property(ui, property_schema, value, path, name, options);
    }
    for (group, members) in &grouped {
        egui::CollapsingHeader::new(localized_group(group))
            .default_open(true)
            .id_salt(("schema_form_group", path, group))
            .show(ui, |ui| {
                for (name, property_schema) in members {
                    changed |= render_property(ui, property_schema, value, path, name, options);
                }
            });
    }

    // Unknown keys: render a raw-JSON fallback so nothing is silently lost.
    let known: std::collections::BTreeSet<&String> = properties.keys().collect();
    let has_unknown = value
        .as_object()
        .is_some_and(|object| object.keys().any(|key| !known.contains(key)));
    if has_unknown {
        egui::CollapsingHeader::new(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "schema-unknown-keys"
        ))
        .id_salt(("schema_form_unknown", path))
        .show(ui, |ui| {
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "schema-unknown-keys-hint"
            ));
            changed |= raw_json_form(ui, value, &format!("{path}.__raw__"), schema, options.epoch);
        });
    }
    changed
}

/// Localizes a group name: `x-ene-ui.group_key` (FTL key) wins; known group
/// codes (`engine`, `voice`, `model`, `runtime`, `connection`) map onto
/// shared keys; anything else (third-party plugins) renders as-is.
fn localized_group(group: &str) -> String {
    if BUILTIN_PROVIDER_GROUP_IDS.contains(&group) {
        return crate::i18n::loader().get(&format!("provider-group-{group}"));
    }
    group.to_string()
}

fn render_property(
    ui: &mut egui::Ui,
    property_schema: &serde_json::Value,
    parent: &mut serde_json::Value,
    parent_path: &str,
    name: &str,
    options: SchemaFormOptions<'_>,
) -> bool {
    let path = format!("{parent_path}.{name}");
    let meta = UiMetadata::from_schema(property_schema);
    if options.show_impact
        && let Some(impact) = meta.impact
    {
        ui.horizontal_wrapped(|ui| {
            ui.label(property_label(name, &meta));
            ui.weak(format!("({})", impact.code()));
        });
    } else {
        ui.label(property_label(name, &meta));
    }
    if let Some(description) = property_description(property_schema, &meta) {
        ui.add(egui::Label::new(egui::RichText::new(description).weak()).wrap());
    }
    if let Some(options_path) = meta.options_path.as_deref() {
        ui.weak(format!(
            "{}: {options_path}",
            i18n_embed_fl::fl!(crate::i18n::loader(), "schema-options")
        ));
    }

    let Some(object) = parent.as_object_mut() else {
        return false;
    };
    let entry = object.entry(name.to_string()).or_insert_with(|| {
        property_schema
            .get("default")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::Null)
    });
    let mut changed = render_value(ui, property_schema, entry, &path, options);

    if let Some(default) = property_schema.get("default")
        && entry != default
        && ui
            .small_button("↺")
            .on_hover_text(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "schema-reset-default"
            ))
            .clicked()
    {
        *entry = default.clone();
        changed = true;
    }
    changed
}

/// Localized property label: `x-ene-ui.label_key` (FTL key) wins, falling
/// back to the raw property name (third-party plugins).
fn property_label(name: &str, meta: &UiMetadata) -> String {
    meta.label_key
        .as_deref()
        .map_or_else(|| name.to_string(), |key| crate::i18n::loader().get(key))
}

/// Localized property description: `x-ene-ui.description_key` (FTL key)
/// wins, falling back to the schema `description` (third-party plugins).
fn property_description(schema: &serde_json::Value, meta: &UiMetadata) -> Option<String> {
    meta.description_key.as_deref().map_or_else(
        || {
            schema
                .get("description")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        },
        |key| Some(crate::i18n::loader().get(key)),
    )
}
fn render_value(
    ui: &mut egui::Ui,
    schema: &serde_json::Value,
    value: &mut serde_json::Value,
    path: &str,
    options: SchemaFormOptions<'_>,
) -> bool {
    // Nullable: `type: ["string", "null"]` or `type: "null"`-adjacent.
    let nullable = schema
        .get("type")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|types| types.iter().any(|t| t == "null"));
    if nullable {
        if value.is_null() {
            let mut set = false;
            ui.horizontal_wrapped(|ui| {
                ui.weak("(null)");
                if ui
                    .small_button(i18n_embed_fl::fl!(crate::i18n::loader(), "schema-null-set"))
                    .clicked()
                {
                    set = true;
                }
            });
            if set {
                *value = schema
                    .get("default")
                    .cloned()
                    .unwrap_or_else(|| serde_json::Value::String(String::new()));
                return true;
            }
            return false;
        }
        let mut clear = false;
        ui.horizontal_wrapped(|ui| {
            if ui
                .small_button(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "schema-null-clear"
                ))
                .clicked()
            {
                clear = true;
            }
        });
        if clear {
            *value = serde_json::Value::Null;
            return true;
        }
    }

    if let Some(one_of) = schema.get("oneOf").and_then(serde_json::Value::as_array) {
        return one_of_form(ui, one_of, value, path, options);
    }

    let type_name = schema
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("string");
    match type_name {
        "object" => schema_object_form(ui, schema, value, path, options),
        "array" => array_form(ui, schema, value, path, options),
        "boolean" => {
            let mut current = value.as_bool().unwrap_or(false);
            let changed = ui.checkbox(&mut current, "").changed();
            if changed {
                *value = serde_json::Value::Bool(current);
            }
            changed
        }
        "integer" | "number" => number_form(ui, schema, value, path, type_name == "integer"),
        _ => string_form(ui, schema, value, path, options.epoch, options.options),
    }
}

fn one_of_form(
    ui: &mut egui::Ui,
    variants: &[serde_json::Value],
    value: &mut serde_json::Value,
    path: &str,
    options: SchemaFormOptions<'_>,
) -> bool {
    let matching = |value: &serde_json::Value| {
        variants.iter().position(|variant| {
            let mut probe = Vec::new();
            super::draft::validate_value(variant, value, path, None, &mut probe);
            probe.is_empty()
        })
    };
    let mut selected = matching(value).unwrap_or(0);
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt(("schema_one_of", path))
            .selected_text(
                variants
                    .get(selected)
                    .map_or_else(|| "—".to_string(), variant_label),
            )
            .show_ui(ui, |ui| {
                for (index, variant) in variants.iter().enumerate() {
                    if ui
                        .selectable_label(index == selected, variant_label(variant))
                        .clicked()
                    {
                        selected = index;
                    }
                }
            });
    });
    if selected != matching(value).unwrap_or(0) {
        changed = true;
        if let Some(variant) = variants.get(selected) {
            *value = variant
                .get("default")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
        }
    }
    if let Some(variant) = variants.get(selected) {
        changed |= render_value(ui, variant, value, path, options);
    }
    changed
}

fn variant_label(variant: &serde_json::Value) -> String {
    variant
        .get("title")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            variant
                .get("type")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| i18n_embed_fl::fl!(crate::i18n::loader(), "schema-variant").to_string())
}

fn array_form(
    ui: &mut egui::Ui,
    schema: &serde_json::Value,
    value: &mut serde_json::Value,
    path: &str,
    options: SchemaFormOptions<'_>,
) -> bool {
    let mut changed = false;
    let item_schema = schema.get("items").cloned();
    let Some(items) = value.as_array_mut() else {
        return false;
    };
    let mut remove: Option<usize> = None;
    for (index, item) in items.iter_mut().enumerate() {
        ui.horizontal_wrapped(|ui| {
            if let Some(item_schema) = &item_schema {
                changed |=
                    render_value(ui, item_schema, item, &format!("{path}[{index}]"), options);
            } else {
                changed |= raw_json_form(
                    ui,
                    item,
                    &format!("{path}[{index}]"),
                    &serde_json::Value::Null,
                    options.epoch,
                );
            }
            if ui.small_button("✕").clicked() {
                remove = Some(index);
            }
        });
    }
    if let Some(index) = remove {
        items.remove(index);
        changed = true;
    }
    if ui
        .small_button("+ add")
        .on_hover_text(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "schema-array-add-hint"
        ))
        .clicked()
    {
        items.push(
            item_schema
                .as_ref()
                .and_then(|s| s.get("default"))
                .cloned()
                .unwrap_or_else(|| serde_json::Value::Null),
        );
        changed = true;
    }
    changed
}

fn number_form(
    ui: &mut egui::Ui,
    schema: &serde_json::Value,
    value: &mut serde_json::Value,
    _path: &str,
    integer: bool,
) -> bool {
    let min = schema.get("minimum").and_then(serde_json::Value::as_f64);
    let max = schema.get("maximum").and_then(serde_json::Value::as_f64);
    let slider = UiMetadata::from_schema(schema).slider;
    if let Some((slider_min, slider_max, slider_step)) = slider {
        let mut current = value.as_f64().unwrap_or(slider_min);
        let bounded = current.clamp(slider_min, slider_max);
        if ui
            .add(egui::Slider::new(&mut current, slider_min..=slider_max).step_by(slider_step))
            .changed()
        {
            let quantized = (current / slider_step).round() * slider_step;
            let quantized = quantized.clamp(slider_min, slider_max);
            *value = if integer {
                serde_json::Value::from(quantized.round() as i64)
            } else {
                serde_json::Value::from(quantized)
            };
            return true;
        }
        ui.weak(format!("{bounded}"));
        return false;
    }
    let formatted = |n: f64| {
        if integer {
            format!("{n:.0}")
        } else {
            format!("{n}")
        }
    };
    let mut text = value.as_f64().map_or_else(String::new, formatted);
    let width = ui.available_width().min(120.0);
    let response = ui.add(egui::TextEdit::singleline(&mut text).desired_width(width));
    if response.changed() && text.trim() != value.as_f64().map_or_else(String::new, formatted) {
        let parsed: Option<f64> = text.trim().parse().ok();
        if integer {
            if let Some(integral) = text.trim().parse::<i64>().ok()
                && let Some(clamped) = clamp_i64(integral, min, max)
            {
                *value = serde_json::Value::from(clamped);
                return true;
            }
        } else if let Some(number) = parsed {
            let lower = min.map_or(number, |m| number.max(m));
            let clamped = max.map_or(lower, |m| lower.min(m));
            *value = serde_json::Value::from(clamped);
            return true;
        }
    }
    false
}

fn clamp_i64(value: i64, min: Option<f64>, max: Option<f64>) -> Option<i64> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "bounded clamp of an already-validated i64; Rust as-casts saturate"
    )]
    let as_f64 = value as f64;
    let lower = min.map_or(as_f64, |m| as_f64.max(m));
    let clamped = max.map_or(lower, |m| lower.min(m));
    if clamped.is_finite() {
        let truncated = clamped as i64;
        Some(truncated)
    } else {
        None
    }
}

fn string_form(
    ui: &mut egui::Ui,
    schema: &serde_json::Value,
    value: &mut serde_json::Value,
    path: &str,
    epoch: u64,
    options: Option<&BTreeMap<String, Vec<(String, String)>>>,
) -> bool {
    let enum_values = schema.get("enum").and_then(serde_json::Value::as_array);
    if let Some(enum_values) = enum_values {
        let labels: Vec<String> = enum_values
            .iter()
            .map(|v| v.as_str().unwrap_or_default().to_string())
            .collect();
        let current = value.as_str().unwrap_or_default().to_string();
        // A stored value outside the current enum (schema updated, value
        // stale) stays visible and untouched until the user picks a new
        // option; auto-replacing it on render would silently edit config.
        let mut options = labels;
        if !current.is_empty() && !options.iter().any(|option| option == &current) {
            options.insert(0, current.clone());
        }
        let mut selected = options
            .iter()
            .position(|option| option == &current)
            .unwrap_or(0);
        let mut changed = false;
        let mut clicked: Option<usize> = None;
        egui::ComboBox::from_id_salt(("schema_enum", path))
            .selected_text(options.get(selected).map_or("—", String::as_str))
            .show_ui(ui, |ui| {
                for (index, option) in options.iter().enumerate() {
                    if ui.selectable_label(index == selected, option).clicked() {
                        selected = index;
                        clicked = Some(index);
                    }
                }
            });
        if let Some(index) = clicked
            && let Some(option) = options.get(index)
            && option != &current
        {
            *value = serde_json::Value::String(option.clone());
            changed = true;
        }
        return changed;
    }

    let meta = UiMetadata::from_schema(schema);
    if meta.options_path.is_some()
        && let Some(entries) = options.and_then(|map| map.get(path))
        && !entries.is_empty()
    {
        let current = value.as_str().unwrap_or_default().to_string();
        let mut display: Vec<(String, String)> = entries.clone();
        if !current.is_empty()
            && !display
                .iter()
                .any(|(_, option_value)| option_value == &current)
        {
            display.insert(0, (current.clone(), current.clone()));
        }
        let mut selected = display
            .iter()
            .position(|(_, option_value)| option_value == &current)
            .unwrap_or(0);
        let mut changed = false;
        let mut clicked: Option<usize> = None;
        egui::ComboBox::from_id_salt(("schema_options", path))
            .selected_text(
                display
                    .get(selected)
                    .map_or("—", |(label, _)| label.as_str()),
            )
            .show_ui(ui, |ui| {
                for (index, (label, _)) in display.iter().enumerate() {
                    if ui
                        .selectable_label(index == selected, label.as_str())
                        .clicked()
                    {
                        selected = index;
                        clicked = Some(index);
                    }
                }
            });
        if let Some(index) = clicked
            && let Some((_, option_value)) = display.get(index)
            && option_value != &current
        {
            *value = serde_json::Value::String(option_value.clone());
            changed = true;
        }
        return changed;
    }
    let is_secret = meta.secret || meta.control.as_deref() == Some("secret");
    if is_secret {
        return secret_form(ui, value, path, epoch);
    }

    let mut current = value.as_str().unwrap_or_default().to_string();
    let mut changed = false;
    let multiline = meta.control.as_deref() == Some("textarea")
        || schema
            .get("format")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|format| format == "longtext");
    let response = if multiline {
        let width = ui.available_width().min(280.0);
        ui.add(
            egui::TextEdit::multiline(&mut current)
                .desired_rows(4)
                .desired_width(width),
        )
    } else {
        let width = ui.available_width().min(220.0);
        ui.add(egui::TextEdit::singleline(&mut current).desired_width(width))
    };
    if response.changed() {
        *value = serde_json::Value::String(current);
        changed = true;
    }
    changed
}

/// Secret field control: the stored value never enters a text buffer.
///
/// The input starts empty; a set secret is shown as a masked hint, and the
/// first keystroke supplies a *replacement* (written straight into the
/// value, never echoed back). Clearing the buffer reverts to unchanged, and
/// a dedicated button deletes the stored secret (writes `null`).
fn secret_form(ui: &mut egui::Ui, value: &mut serde_json::Value, path: &str, epoch: u64) -> bool {
    let buffer_id = egui::Id::new(("schema_secret_buffer", path, epoch));
    let mut buffer = ui.data_mut(|data| data.get_temp::<String>(buffer_id).unwrap_or_default());
    let is_placeholder = value.as_str() == Some(super::draft::SECRET_PLACEHOLDER);
    let is_set = is_placeholder || value.as_str().is_some_and(|current| !current.is_empty());
    let mut changed = false;
    let content = |ui: &mut egui::Ui| {
        if is_set && buffer.is_empty() {
            ui.weak("••••••••");
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "schema-secret-set-hint"
            ));
        }
        let width = ui.available_width().min(220.0);
        let response = ui.add(
            egui::TextEdit::singleline(&mut buffer)
                .password(true)
                .desired_width(width),
        );
        if response.changed() {
            ui.data_mut(|data| {
                data.insert_temp(buffer_id, buffer.clone());
            });
            let next = secret_input_next(value, &buffer);
            if *value != next {
                *value = next;
                changed = true;
            }
        }
        if is_set
            && ui
                .small_button(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "schema-secret-clear"
                ))
                .on_hover_text(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "schema-secret-clear-hint"
                ))
                .clicked()
        {
            // Deletion for a string-typed secret: an explicit empty value
            // persists (merge never restores empty strings), while `null`
            // stays valid for nullable fields.
            *value = serde_json::Value::String(String::new());
            ui.data_mut(|data| {
                data.insert_temp(buffer_id, String::new());
            });
            changed = true;
        }
    };
    ui.horizontal_wrapped(content);
    changed
}

/// Pure value transition for a secret field given the current (redacted)
/// value and the text buffer.
///
/// - non-empty buffer → replacement (user input wins);
/// - empty buffer over a real (previously typed) value → back to
///   [`super::draft::SECRET_PLACEHOLDER`] (unchanged), never an empty string;
/// - empty buffer over a placeholder → unchanged;
/// - `null` stays `null` (explicit deletion).
fn secret_input_next(value: &serde_json::Value, buffer: &str) -> serde_json::Value {
    if !buffer.is_empty() {
        return serde_json::Value::String(buffer.to_string());
    }
    if value.is_null() {
        return serde_json::Value::Null;
    }
    if value.as_str() == Some(super::draft::SECRET_PLACEHOLDER) {
        return value.clone();
    }
    serde_json::Value::String(super::draft::SECRET_PLACEHOLDER.to_string())
}

/// Raw JSON fallback: edits the full JSON text of `value`.
///
/// Commits only when the text parses; parse errors are shown without
/// touching the value. Used for unknown schemas and unknown-key blocks.
pub fn raw_json_form(
    ui: &mut egui::Ui,
    value: &mut serde_json::Value,
    path: &str,
    schema: &serde_json::Value,
    epoch: u64,
) -> bool {
    let id = egui::Id::new(("schema_raw", path, epoch));
    let defs = schema
        .get("$defs")
        .or_else(|| schema.get("definitions"))
        .and_then(serde_json::Value::as_object);
    let redacted_text =
        serde_json::to_string_pretty(&redact_by_schema(value, schema, defs)).unwrap_or_default();
    let mut text = ui.data_mut(|data| data.get_temp::<String>(id).unwrap_or(redacted_text));
    let mut error: Option<String> =
        ui.data_mut(|data| data.get_temp::<String>(egui::Id::new(("schema_raw_err", path))));
    let width = ui.available_width().min(320.0);
    let response = ui.add(
        egui::TextEdit::multiline(&mut text)
            .code_editor()
            .desired_rows(5)
            .desired_width(width),
    );
    if response.changed() {
        ui.data_mut(|data| {
            data.insert_temp(id, text.clone());
        });
    }
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        if ui
            .small_button(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "schema-raw-apply"
            ))
            .clicked()
        {
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(mut parsed) => {
                    unredact_by_schema(value, &mut parsed, schema, defs);
                    *value = parsed;
                    changed = true;
                    error = None;
                    ui.data_mut(|data| {
                        data.insert_temp(egui::Id::new(("schema_raw_err", path)), String::new());
                    });
                }
                Err(e) => {
                    error = Some(e.to_string());
                    ui.data_mut(|data| {
                        data.insert_temp(egui::Id::new(("schema_raw_err", path)), e.to_string());
                    });
                }
            }
        }
        if let Some(error) = error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
    });
    changed
}

/// Resolves a `$ref`-only schema node against `defs`, transitively.
fn resolve_schema_ref<'a>(
    schema: &'a serde_json::Value,
    defs: Option<&'a serde_json::Map<String, serde_json::Value>>,
) -> &'a serde_json::Value {
    let Some(reference) = schema.get("$ref").and_then(serde_json::Value::as_str) else {
        return schema;
    };
    let name = reference
        .strip_prefix("#/$defs/")
        .or_else(|| reference.strip_prefix("#/definitions/"));
    if let Some(name) = name
        && let Some(resolved) = defs.and_then(|defs| defs.get(name))
    {
        return resolve_schema_ref(resolved, defs);
    }
    schema
}

fn is_secret_schema(schema: &serde_json::Value) -> bool {
    let meta = UiMetadata::from_schema(schema);
    meta.secret || meta.control.as_deref() == Some("secret")
}

/// Recursive schema-guided redaction: every secret leaf — nested objects,
/// **all** array elements, `oneOf` variants, `$ref` targets — becomes the
/// placeholder. Non-secret values pass through untouched.
fn redact_by_schema(
    value: &serde_json::Value,
    schema: &serde_json::Value,
    defs: Option<&serde_json::Map<String, serde_json::Value>>,
) -> serde_json::Value {
    let schema = resolve_schema_ref(schema, defs);
    if is_secret_schema(schema) {
        return if value.as_str().is_some_and(|string| !string.is_empty()) {
            serde_json::Value::String(super::draft::SECRET_PLACEHOLDER.to_string())
        } else {
            value.clone()
        };
    }
    match value {
        serde_json::Value::Object(object) => {
            let mut redacted = object.clone();
            if let Some(properties) = schema
                .get("properties")
                .and_then(serde_json::Value::as_object)
            {
                for (key, child) in &mut redacted {
                    if let Some(child_schema) = properties.get(key) {
                        *child = redact_by_schema(child, child_schema, defs);
                    }
                }
            }
            if let Some(one_of) = schema.get("oneOf").and_then(serde_json::Value::as_array) {
                for variant in one_of {
                    let candidate = redact_by_schema(
                        &serde_json::Value::Object(redacted.clone()),
                        variant,
                        defs,
                    );
                    if let Some(object) = candidate.as_object() {
                        redacted.clone_from(object);
                    }
                }
            }
            serde_json::Value::Object(redacted)
        }
        serde_json::Value::Array(items) => {
            let item_schema = schema.get("items");
            serde_json::Value::Array(
                items
                    .iter()
                    .map(|item| match item_schema {
                        Some(item_schema) => redact_by_schema(item, item_schema, defs),
                        None => item.clone(),
                    })
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

/// Recursive counterpart of [`redact_by_schema`]: restores the original
/// value wherever the parsed text still holds the placeholder, so an
/// unchanged secret survives a raw-JSON round trip while a user-supplied
/// replacement wins.
fn unredact_by_schema(
    old: &serde_json::Value,
    new: &mut serde_json::Value,
    schema: &serde_json::Value,
    defs: Option<&serde_json::Map<String, serde_json::Value>>,
) {
    let schema = resolve_schema_ref(schema, defs);
    if is_secret_schema(schema) {
        if new.as_str() == Some(super::draft::SECRET_PLACEHOLDER) && old.is_string() {
            *new = old.clone();
        }
        return;
    }
    match new {
        serde_json::Value::Object(object) => {
            if let Some(properties) = schema
                .get("properties")
                .and_then(serde_json::Value::as_object)
            {
                for (key, child) in object.iter_mut() {
                    if let Some(child_schema) = properties.get(key)
                        && let Some(old_child) = old.get(key)
                    {
                        unredact_by_schema(old_child, child, child_schema, defs);
                    }
                }
            }
            if let Some(one_of) = schema.get("oneOf").and_then(serde_json::Value::as_array) {
                for variant in one_of {
                    unredact_by_schema(old, new, variant, defs);
                }
            }
        }
        serde_json::Value::Array(items) => {
            if let Some(old_items) = old.as_array()
                && let Some(item_schema) = schema.get("items")
            {
                for (index, item) in items.iter_mut().enumerate() {
                    if let Some(old_item) = old_items.get(index) {
                        unredact_by_schema(old_item, item, item_schema, defs);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Returns the schema constructs this renderer does not understand, walking
/// `schema` recursively. The advanced-page coverage test asserts every
/// registered settings schema has an empty result, which is the
/// "every schema leaf is reachable from the GUI" guarantee.
#[must_use]
pub fn unsupported_schema_constructs(schema: &serde_json::Value) -> Vec<String> {
    let mut unsupported = Vec::new();
    walk_schema(schema, "#", &mut unsupported);
    unsupported
}

fn walk_schema(schema: &serde_json::Value, path: &str, unsupported: &mut Vec<String>) {
    if schema.get("$ref").is_some() {
        return;
    }
    if let Some(one_of) = schema.get("oneOf").and_then(serde_json::Value::as_array) {
        for (index, variant) in one_of.iter().enumerate() {
            walk_schema(variant, &format!("{path}/oneOf/{index}"), unsupported);
        }
        return;
    }
    let types = match schema.get("type") {
        Some(serde_json::Value::String(name)) => vec![name.as_str()],
        Some(serde_json::Value::Array(types)) => {
            types.iter().filter_map(serde_json::Value::as_str).collect()
        }
        _ => Vec::new(),
    };
    for type_name in types {
        if !matches!(
            type_name,
            "object" | "array" | "string" | "number" | "integer" | "boolean" | "null"
        ) {
            unsupported.push(format!("{path}: type {type_name}"));
        }
    }
    if let Some(properties) = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
    {
        for (name, property_schema) in properties {
            walk_schema(
                property_schema,
                &format!("{path}/properties/{name}"),
                unsupported,
            );
        }
    }
    if let Some(items) = schema.get("items") {
        walk_schema(items, &format!("{path}/items"), unsupported);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ui_metadata_parses_extensions() {
        let schema = json!({
            "type": "string",
            "x-ene-ui": {
                "group": "tuning",
                "order": 2.0,
                "control": "secret",
                "advanced": true,
                "impact": "plugin_restart",
                "options_path": "voices"
            }
        });
        let meta = UiMetadata::from_schema(&schema);
        assert_eq!(meta.group.as_deref(), Some("tuning"));
        assert_eq!(meta.order, Some(2.0));
        assert_eq!(meta.control.as_deref(), Some("secret"));
        assert!(meta.is_advanced());
        assert_eq!(meta.impact, Some(FieldImpact::PluginRestart));
        assert_eq!(meta.options_path.as_deref(), Some("voices"));
        assert!(meta.secret);
    }

    #[test]
    fn legacy_secret_flag_is_recognized() {
        let meta = UiMetadata::from_schema(&json!({"type": "string", "x-ene-secret": true}));
        assert!(meta.secret);
    }

    #[test]
    fn profiles_schema_resolves_extension() {
        let schema = json!({
            "type": "object",
            "x-ene-profiles-schema": {"type": "object", "properties": {"voices_path": {"type": "string"}}}
        });
        assert!(profiles_schema(&schema).is_some());
        let fallback = json!({
            "type": "object",
            "properties": {"profiles": {"type": "object"}}
        });
        assert!(profiles_schema(&fallback).is_some());
        assert!(profiles_schema(&json!({"type": "object"})).is_none());
    }

    #[test]
    fn unsupported_constructs_are_detected() {
        let schema = json!({
            "type": "object",
            "properties": {
                "ok": {"type": "string"},
                "weird": {"type": "uri-reference"}
            }
        });
        let issues = unsupported_schema_constructs(&schema);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("uri-reference"));
    }

    #[test]
    fn every_registered_settings_schema_is_renderable() {
        let sections =
            ene_config::config::registered_schemas_for(ene_config::ConfigTarget::Settings);
        assert!(
            !sections.is_empty(),
            "the settings schema registry must be populated"
        );
        for (key, entry) in sections {
            let schema = serde_json::to_value(&entry.schema).expect("schema serializes");
            let issues = unsupported_schema_constructs(&schema);
            assert!(
                issues.is_empty(),
                "section `{key}` has schema constructs the generic form cannot render: {issues:?}"
            );
        }
    }

    #[test]
    fn narrow_provider_schema_form_stays_within_content_width() {
        let schema = json!({
            "type": "object",
            "properties": {
                "api_key": {
                    "oneOf": [
                        {"type": "string"},
                        {"type": "object", "properties": {
                            "source": {"type": "string", "enum": ["inline", "env", "auto"]},
                            "inline": {"type": "string"},
                            "env": {"type": "string"}
                        }}
                    ],
                    "x-ene-secret": true,
                    "description": "API key or credential descriptor"
                },
                "base_url": {
                    "type": "string",
                    "description": "API base URL override for a compatible speech service endpoint",
                    "x-ene-ui": {"group": "connection", "impact": "runtime_reload"}
                },
                "model": {
                    "type": "string",
                    "enum": ["tts-1", "tts-1-hd"],
                    "x-ene-ui": {"group": "voice", "impact": "runtime_reload"}
                },
                "voice": {
                    "type": "string",
                    "enum": ["alloy", "echo", "fable", "onyx", "nova", "shimmer"],
                    "x-ene-ui": {"group": "voice", "impact": "runtime_reload"}
                },
                "speed": {
                    "type": "number",
                    "minimum": 0.25,
                    "maximum": 4.0,
                    "x-ene-ui": {"group": "voice", "impact": "runtime_reload"}
                }
            }
        });
        let mut value = json!({
            "api_key": "",
            "base_url": "https://api.openai.com/v1",
            "model": "tts-1",
            "voice": "alloy",
            "speed": 1.0
        });
        let context = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(560.0, 700.0),
            )),
            ..Default::default()
        };
        let mut overflow = 0.0_f32;
        let _output = context.run_ui(input, |ui| {
            let content_right = ui.max_rect().right();
            ui.indent("provider_details", |ui| {
                schema_object_form(
                    ui,
                    &schema,
                    &mut value,
                    "plugins.list.openai-tts.config",
                    SchemaFormOptions {
                        show_advanced: true,
                        show_impact: true,
                        epoch: 0,
                        options: None,
                    },
                );
            });
            overflow = (ui.min_rect().right() - content_right).max(0.0);
        });
        assert!(
            overflow <= 0.5,
            "narrow provider form overflowed by {overflow} points"
        );
    }

    #[test]
    fn redaction_covers_nested_secrets_and_all_array_elements() {
        let schema = json!({
            "type": "object",
            "properties": {
                "api_key": {"type": "string", "x-ene-secret": true},
                "client": {
                    "type": "object",
                    "properties": {
                        "token": {"type": "string", "x-ene-ui": {"control": "secret"}}
                    }
                },
                "accounts": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string"},
                            "secret": {"type": "string", "x-ene-secret": true}
                        }
                    }
                }
            }
        });
        let value = json!({
            "api_key": "sk-super-secret",
            "client": {"token": "nested-secret"},
            "accounts": [
                {"id": "a", "secret": "one"},
                {"id": "b", "secret": "two"}
            ]
        });
        let redacted = redact_by_schema(&value, &schema, None);
        let redacted_text = serde_json::to_string(&redacted).unwrap();
        assert!(
            !redacted_text.contains("sk-super-secret"),
            "the stored secret must never appear in raw JSON text: {redacted_text}"
        );
        assert!(!redacted_text.contains("nested-secret"));
        assert!(
            !redacted_text.contains("one") && !redacted_text.contains("two"),
            "every array element is redacted, not just the first: {redacted_text}"
        );
        assert_eq!(
            redacted["client"]["token"],
            super::super::draft::SECRET_PLACEHOLDER
        );
        assert_eq!(
            redacted["accounts"][0]["secret"],
            super::super::draft::SECRET_PLACEHOLDER
        );
        assert_eq!(
            redacted["accounts"][1]["secret"],
            super::super::draft::SECRET_PLACEHOLDER
        );

        let mut parsed: serde_json::Value =
            serde_json::from_str(&redacted_text).expect("redacted text parses");
        unredact_by_schema(&value, &mut parsed, &schema, None);
        assert_eq!(parsed["api_key"], "sk-super-secret");
        assert_eq!(parsed["accounts"][1]["secret"], "two");

        let mut replaced: serde_json::Value =
            serde_json::from_str(&redacted_text).expect("redacted text parses");
        replaced["api_key"] = json!("sk-new");
        unredact_by_schema(&value, &mut replaced, &schema, None);
        assert_eq!(replaced["api_key"], "sk-new");
    }

    #[test]
    fn secret_input_clear_returns_to_unchanged() {
        let placeholder = json!(super::super::draft::SECRET_PLACEHOLDER);
        assert_eq!(secret_input_next(&placeholder, "sk-new"), json!("sk-new"));
        assert_eq!(secret_input_next(&json!("sk-new"), ""), placeholder);
        assert_eq!(secret_input_next(&placeholder, ""), placeholder);
        assert_eq!(secret_input_next(&json!(null), ""), json!(null));
    }

    #[test]
    fn secret_placeholder_is_collision_safe() {
        assert_ne!(super::super::draft::SECRET_PLACEHOLDER, "sk-");
        assert!(super::super::draft::SECRET_PLACEHOLDER.starts_with('\u{0}'));
    }
}
