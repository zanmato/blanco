use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    Styled, Window, div, px,
};
use gpui_component::{
    ActiveTheme, StyledExt, h_flex,
    input::{Input, InputState},
    v_flex,
};
use std::collections::HashMap;

use crate::sql_statement_parser::QueryParameter;

#[derive(Clone, Debug)]
pub struct ParameterInput {
    pub label: String,            // e.g., "$1" or ":user_id"
    pub raw_text: String,         // For display (e.g., "$1")
    pub byte_offsets: Vec<usize>, // All byte offsets where this parameter appears (sorted descending for replacement)
    pub byte_length: usize,       // Length in bytes of the parameter text
    pub input: Entity<InputState>,
}

pub struct ParameterForm {
    focus_handle: FocusHandle,
    original_query: String,
    parameters: Vec<ParameterInput>,
}

impl ParameterForm {
    pub fn new(
        original_query: String,
        parameters: Vec<QueryParameter>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        // Group parameters by label (deduplication)
        // Each unique label gets one input, but we track all byte offsets for replacement
        let mut param_map: HashMap<String, ParameterInput> = HashMap::new();

        for param in parameters {
            let label = match &param.style {
                crate::sql_statement_parser::ParameterStyle::Positional(n) => format!("${}", n),
                crate::sql_statement_parser::ParameterStyle::Named(name) => {
                    format!(":{}", name)
                }
            };

            let byte_offset = param.byte_offset;
            let byte_length = param.raw_text.len();

            if let Some(existing) = param_map.get_mut(&label) {
                // Add this offset to the existing parameter
                existing.byte_offsets.push(byte_offset);
            } else {
                // Create new parameter input
                let input = cx.new(|cx| {
                    InputState::new(window, cx).placeholder(&format!("Value for {}", label))
                });

                param_map.insert(
                    label.clone(),
                    ParameterInput {
                        label,
                        raw_text: param.raw_text.clone(),
                        byte_offsets: vec![byte_offset],
                        byte_length,
                        input,
                    },
                );
            }
        }

        // Convert map to vec and sort by first offset (ascending for display)
        let mut param_inputs: Vec<ParameterInput> = param_map.into_values().collect();
        param_inputs.sort_by_key(|p| p.byte_offsets[0]);

        Self {
            focus_handle,
            original_query,
            parameters: param_inputs,
        }
    }

    pub fn get_substituted_query(&self, cx: &App) -> String {
        // Collect all replacements with their original offsets
        let mut replacements: Vec<(usize, usize, String)> = Vec::new();
        for param_input in &self.parameters {
            let value = param_input.input.read(cx).value().to_string();
            for &byte_offset in &param_input.byte_offsets {
                replacements.push((byte_offset, param_input.byte_length, value.clone()));
            }
        }

        // Sort by offset descending (so replacements don't affect earlier offsets)
        replacements.sort_by_key(|(offset, _, _)| std::cmp::Reverse(*offset));

        tracing::info!(
            "get_substituted_query: original_query={}, replacements={:?}",
            self.original_query,
            replacements
        );

        // Apply replacements in descending offset order
        let mut result = self.original_query.clone();
        for (byte_offset, byte_length, value) in replacements {
            if byte_offset + byte_length <= result.len() {
                let before = &result[..byte_offset];
                let after = &result[byte_offset + byte_length..];
                let new_result = format!("{}{}{}", before, value, after);
                tracing::info!(
                    "Replacing at offset {}: len={}, value='{}', result='{}'",
                    byte_offset,
                    byte_length,
                    value,
                    new_result
                );
                result = new_result;
            } else {
                tracing::warn!(
                    "Skipping invalid replacement: offset={}, len={}, result_len={}",
                    byte_offset,
                    byte_length,
                    result.len()
                );
            }
        }

        result
    }
}

impl Focusable for ParameterForm {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ParameterForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_4()
            .children(self.parameters.iter().map(|param| {
                h_flex()
                    .gap_4()
                    .font_family(cx.theme().mono_font_family.clone())
                    .child(div().text_sm().child(param.label.clone()))
                    .child(Input::new(&param.input))
                    .into_any_element()
            }))
    }
}
