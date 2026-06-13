use iced::widget::{Button, Column, button, checkbox, column, container, row, scrollable, text, text_input};
use iced::{Element, Length};

use crate::models::calendar::Calendar;
use crate::models::event::CalendarEvent;
use crate::models::task::CalendarTask;
use crate::ui::app::Message;

#[derive(Debug, Clone)]
pub enum CalendarMessage {
    CalendarSelected(String),
    EventSelected(String),
    TaskSelected(String),
    NewEvent,
    NewTask,
    SaveEvent,
    SaveTask,
    DeleteEvent,
    DeleteTask,
    CancelEdit,
    TitleChanged(String),
    StartChanged(String),
    EndChanged(String),
    AllDayChanged(bool),
    DescriptionChanged(String),
    LocationChanged(String),
    DueChanged(String),
    StatusChanged(String),
    ImportIcs,
    ExportIcs,
}

impl CalendarMessage {
    pub fn into_message(self) -> Message {
        Message::Calendar(self)
    }
}

#[derive(Debug, Default)]
pub struct CalendarState {
    pub calendars: Vec<Calendar>,
    pub events: Vec<CalendarEvent>,
    pub tasks: Vec<CalendarTask>,
    pub selected_calendar: Option<String>,
    pub selected_event: Option<CalendarEvent>,
    pub selected_task: Option<CalendarTask>,
    pub editing_event: bool,
    pub editing_task: bool,
    pub edit_title: String,
    pub edit_start: String,
    pub edit_end: String,
    pub edit_all_day: bool,
    pub edit_description: String,
    pub edit_location: String,
    pub edit_due: String,
    pub edit_status: String,
}

pub fn view(state: &CalendarState) -> Element<'_, Message> {
    let sidebar = calendar_list(state);
    let event_list = event_list_view(state);
    let task_list = task_list_view(state);
    let detail = detail_view(state);

    row![
        container(sidebar).width(Length::Fixed(160.0)),
        container(event_list).width(Length::Fixed(200.0)),
        container(task_list).width(Length::Fixed(200.0)),
        container(detail).width(Length::Fill),
    ]
    .spacing(8)
    .padding(8)
    .into()
}

fn calendar_list(state: &CalendarState) -> Element<'_, Message> {
    let mut col = Column::new().spacing(4).padding(8);

    for cal in &state.calendars {
        let selected = state.selected_calendar.as_deref() == Some(cal.id.as_str());
        let btn = Button::new(text(&cal.name).size(13))
            .on_press(CalendarMessage::CalendarSelected(cal.id.clone()).into_message());
        let btn = if selected {
            btn.style(|theme: &iced::Theme, status| {
                let mut style = button::primary(theme, status);
                style.background = Some(iced::Background::Color(theme.palette().primary));
                style
            })
        } else {
            btn
        };
        col = col.push(btn);
    }

    scrollable(col).into()
}

fn event_list_view(state: &CalendarState) -> Element<'_, Message> {
    let mut col = Column::new().spacing(4).padding(8);
    col = col.push(text("Events").size(14));

    for event in &state.events {
        let selected = state
            .selected_event
            .as_ref()
            .map(|e| e.id == event.id)
            .unwrap_or(false);
        let label = format!("{}\n{}", event.title, event.start.format("%Y-%m-%d %H:%M"));
        let btn = Button::new(text(label).size(11))
            .on_press(CalendarMessage::EventSelected(event.id.clone()).into_message());
        let btn = if selected {
            btn.style(|theme: &iced::Theme, status| {
                let mut style = button::primary(theme, status);
                style.background = Some(iced::Background::Color(theme.palette().primary));
                style
            })
        } else {
            btn
        };
        col = col.push(btn);
    }

    scrollable(col).into()
}

fn task_list_view(state: &CalendarState) -> Element<'_, Message> {
    let mut col = Column::new().spacing(4).padding(8);
    col = col.push(text("Tasks").size(14));

    for task in &state.tasks {
        let selected = state
            .selected_task
            .as_ref()
            .map(|t| t.id == task.id)
            .unwrap_or(false);
        let due = task
            .due
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "no due".to_string());
        let label = format!("{}\n{}", task.title, due);
        let btn = Button::new(text(label).size(11))
            .on_press(CalendarMessage::TaskSelected(task.id.clone()).into_message());
        let btn = if selected {
            btn.style(|theme: &iced::Theme, status| {
                let mut style = button::primary(theme, status);
                style.background = Some(iced::Background::Color(theme.palette().primary));
                style
            })
        } else {
            btn
        };
        col = col.push(btn);
    }

    scrollable(col).into()
}

fn detail_view(state: &CalendarState) -> Element<'_, Message> {
    if state.editing_event || state.editing_task {
        return edit_view(state);
    }

    let mut col = Column::new().spacing(8).padding(8);
    col = col.push(
        row![
            Button::new("New event").on_press(CalendarMessage::NewEvent.into_message()),
            Button::new("New task").on_press(CalendarMessage::NewTask.into_message()),
            Button::new("Import ICS").on_press(CalendarMessage::ImportIcs.into_message()),
            Button::new("Export ICS").on_press(CalendarMessage::ExportIcs.into_message()),
        ]
        .spacing(8),
    );

    if let Some(event) = &state.selected_event {
        col = col
            .push(text(&event.title).size(16))
            .push(text(format!("Start: {}", event.start.format("%Y-%m-%d %H:%M"))).size(12))
            .push(text(format!("End: {}", event.end.format("%Y-%m-%d %H:%M"))).size(12))
            .push(text(format!("Location: {}", event.location)).size(12))
            .push(text(&event.description).size(12))
            .push(
                row![
                    Button::new("Edit").on_press(CalendarMessage::NewEvent.into_message()),
                    Button::new("Delete")
                        .on_press(CalendarMessage::DeleteEvent.into_message()),
                ]
                .spacing(8),
            );
    } else if let Some(task) = &state.selected_task {
        col = col
            .push(text(&task.title).size(16))
            .push(text(format!("Status: {}", task.status)).size(12))
            .push(text(format!("Description: {}", task.description)).size(12))
            .push(
                row![
                    Button::new("Edit").on_press(CalendarMessage::NewTask.into_message()),
                    Button::new("Delete")
                        .on_press(CalendarMessage::DeleteTask.into_message()),
                ]
                .spacing(8),
            );
    } else {
        col = col.push(text("Select an event or task").size(12));
    }

    scrollable(col).into()
}

fn edit_view(state: &CalendarState) -> Element<'_, Message> {
    let mut col = column![
        text_input("Title", &state.edit_title)
            .on_input(|v| CalendarMessage::TitleChanged(v).into_message()),
        text_input("Description", &state.edit_description)
            .on_input(|v| CalendarMessage::DescriptionChanged(v).into_message()),
        text_input("Location", &state.edit_location)
            .on_input(|v| CalendarMessage::LocationChanged(v).into_message()),
    ]
    .spacing(8)
    .padding(8);

    if state.editing_event {
        col = col
            .push(text("Event").size(16))
            .push(
                text_input("Start (ISO 8601)", &state.edit_start)
                    .on_input(|v| CalendarMessage::StartChanged(v).into_message()),
            )
            .push(
                text_input("End (ISO 8601)", &state.edit_end)
                    .on_input(|v| CalendarMessage::EndChanged(v).into_message()),
            )
            .push(
                checkbox(state.edit_all_day)
                    .label("All day")
                    .on_toggle(|v| CalendarMessage::AllDayChanged(v).into_message()),
            )
            .push(
                row![
                    Button::new("Save").on_press(CalendarMessage::SaveEvent.into_message()),
                    Button::new("Cancel").on_press(CalendarMessage::CancelEdit.into_message()),
                ]
                .spacing(8),
            );
    } else {
        col = col
            .push(text("Task").size(16))
            .push(
                text_input("Due (ISO 8601)", &state.edit_due)
                    .on_input(|v| CalendarMessage::DueChanged(v).into_message()),
            )
            .push(
                text_input("Status", &state.edit_status)
                    .on_input(|v| CalendarMessage::StatusChanged(v).into_message()),
            )
            .push(
                row![
                    Button::new("Save").on_press(CalendarMessage::SaveTask.into_message()),
                    Button::new("Cancel").on_press(CalendarMessage::CancelEdit.into_message()),
                ]
                .spacing(8),
            );
    }

    col.into()
}
