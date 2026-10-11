use chrono::{Datelike, FixedOffset, Local, NaiveDate, NaiveTime, Offset, Timelike, Utc};
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, Styled, Window, div, prelude::FluentBuilder as _,
};
use gpui_base::actions::Confirm;
use gpui_component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    calendar::{Calendar, CalendarState, Date},
    h_flex,
    time_field::{TimeField, TimeFieldState, TimePrecision},
    v_flex,
};

use super::temporal::{TemporalShape, TemporalValue};
use crate::app::{EditNextCell, EditPrevCell};
use crate::settings::TimeZoneDisplay;

pub enum DateTimePickerEvent {
    Pick(TemporalValue),
    Cancel,
}

/// Popover content for choosing a date, a time or both for a cell.
///
/// Zoned timestamps are edited as wall-clock time in the zone the grid shows
/// them in, so what the picker opens on matches the cell.
pub struct DateTimePicker {
    shape: TemporalShape,
    time_zone: TimeZoneDisplay,
    calendar: Option<Entity<CalendarState>>,
    time_field: Option<Entity<TimeFieldState>>,
    fallback_date: NaiveDate,
    /// The opening time with its fractional seconds, which the time field
    /// cannot show. Kept when the seconds are left alone, so changing only the
    /// date does not truncate the time.
    original_time: Option<NaiveTime>,
    offset: FixedOffset,
}

impl EventEmitter<DateTimePickerEvent> for DateTimePicker {}

impl DateTimePicker {
    pub fn new(
        shape: TemporalShape,
        initial: Option<TemporalValue>,
        time_zone: TimeZoneDisplay,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let now = TemporalValue::now(shape, time_zone);
        let initial = initial.map(|value| value.in_time_zone(time_zone));
        let date = initial
            .and_then(|value| value.date())
            .or_else(|| now.date())
            .unwrap_or_default();
        let original_time = initial.and_then(|value| value.time());
        let time = original_time.or_else(|| now.time()).unwrap_or_default();
        let offset = initial
            .and_then(|value| value.offset())
            .or_else(|| now.offset())
            .unwrap_or_else(|| Utc.fix());

        let calendar = shape.has_date().then(|| {
            cx.new(|cx| {
                let today_year = Local::now().year();
                let year = date.year();
                let mut state = CalendarState::new(window, cx)
                    .year_range((year.min(today_year) - 50, year.max(today_year) + 51));
                state.set_date(Date::Single(Some(date)), window, cx);
                state
            })
        });
        let time_field = shape.has_time().then(|| {
            cx.new(|cx| {
                let mut state = TimeFieldState::new(window, cx).precision(TimePrecision::Second);
                state.set_time(TimePrecision::Second.truncate(time), window, cx);
                state
            })
        });

        Self {
            shape,
            time_zone,
            calendar,
            time_field,
            fallback_date: date,
            original_time,
            offset,
        }
    }

    /// The control keyboard input should land in when the picker opens: the
    /// time field when there is one, since the calendar has no keyboard model.
    pub fn initial_focus(&self, cx: &App) -> Option<FocusHandle> {
        match (&self.time_field, &self.calendar) {
            (Some(time_field), _) => Some(time_field.read(cx).focus_handle(cx)),
            (None, Some(calendar)) => Some(calendar.read(cx).focus_handle.clone()),
            (None, None) => None,
        }
    }

    fn selected_value(&self, cx: &App) -> Option<TemporalValue> {
        let date = self
            .calendar
            .as_ref()
            .and_then(|calendar| calendar.read(cx).date().start())
            .unwrap_or(self.fallback_date);
        let time = self
            .time_field
            .as_ref()
            .map(|time_field| {
                let edited = time_field.read(cx).time();
                self.original_time
                    .filter(|original| original.with_nanosecond(0) == Some(edited))
                    .unwrap_or(edited)
            })
            .unwrap_or_default();
        TemporalValue::from_parts(self.shape, date, time, self.offset)
    }

    #[cfg(test)]
    pub fn select(
        &mut self,
        date: NaiveDate,
        time: NaiveTime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(calendar) = &self.calendar {
            calendar.update(cx, |calendar, cx| {
                calendar.set_date(Date::Single(Some(date)), window, cx)
            });
        }
        if let Some(time_field) = &self.time_field {
            time_field.update(cx, |time_field, cx| time_field.set_time(time, window, cx));
        }
    }

    pub(crate) fn apply(&mut self, cx: &mut Context<Self>) {
        match self.selected_value(cx) {
            Some(value) => cx.emit(DateTimePickerEvent::Pick(value)),
            None => tracing::warn!("The picked date and time do not form a valid value"),
        }
    }

    pub(crate) fn pick_now(&mut self, cx: &mut Context<Self>) {
        cx.emit(DateTimePickerEvent::Pick(TemporalValue::now(
            self.shape,
            self.time_zone,
        )));
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        cx.emit(DateTimePickerEvent::Cancel);
    }
}

impl Render for DateTimePicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let zone_label =
            (self.shape == TemporalShape::TimestampWithTimeZone).then(|| self.offset.to_string());

        v_flex()
            .key_context("DateTimePicker")
            .gap_3()
            .on_action(cx.listener(|picker, _: &Confirm, _, cx| picker.apply(cx)))
            // The picker floats over the cell editor, whose Tab bindings would
            // otherwise move the edit to the next cell underneath it.
            .on_action(|_: &EditNextCell, _, cx| cx.stop_propagation())
            .on_action(|_: &EditPrevCell, _, cx| cx.stop_propagation())
            .when_some(self.calendar.clone(), |this, calendar| {
                this.child(Calendar::new(&calendar).small().border_0().p_0())
            })
            .when_some(self.time_field.clone(), |this, time_field| {
                this.child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("Time"),
                        )
                        .child(TimeField::new(&time_field).small())
                        .when_some(zone_label, |this, zone_label| {
                            this.child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(zone_label),
                            )
                        }),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("date-time-picker-now")
                            .label("Now")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|picker, _, _, cx| picker.pick_now(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("date-time-picker-cancel")
                            .label("Cancel")
                            .small()
                            .on_click(cx.listener(|picker, _, _, cx| picker.cancel(cx))),
                    )
                    .child(
                        Button::new("date-time-picker-apply")
                            .label("Apply")
                            .primary()
                            .small()
                            .on_click(cx.listener(|picker, _, _, cx| picker.apply(cx))),
                    ),
            )
    }
}
