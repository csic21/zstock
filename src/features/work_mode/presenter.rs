#[derive(Debug, Clone, PartialEq)]
pub enum SemanticEvent {
    QuotesUpdated { count: usize },
    ScanCompleted { count: usize },
    PersistenceFailed { message: String },
}

pub trait Presenter {
    fn status(&self, event: &SemanticEvent) -> String;
}

pub struct NormalPresenter;

impl Presenter for NormalPresenter {
    fn status(&self, event: &SemanticEvent) -> String {
        match event {
            SemanticEvent::QuotesUpdated { count } => format!("行情已更新 · {count} 只"),
            SemanticEvent::ScanCompleted { count } => format!("规则扫描完成 · {count} 只"),
            SemanticEvent::PersistenceFailed { message } => format!("保存失败：{message}"),
        }
    }
}

pub struct WorkPresenter;

impl Presenter for WorkPresenter {
    fn status(&self, event: &SemanticEvent) -> String {
        match event {
            SemanticEvent::QuotesUpdated { count } => format!("Synced {count} services"),
            SemanticEvent::ScanCompleted { count } => format!("Scan complete · {count}"),
            SemanticEvent::PersistenceFailed { message } => format!("Save failed · {message}"),
        }
    }
}
