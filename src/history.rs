#[derive(Default)]
pub(crate) struct CommandHistory {
    entries: Vec<String>,
    position: Option<usize>,
    draft: Option<String>,
}

impl CommandHistory {
    pub(crate) fn push(&mut self, command: String) {
        self.entries.push(command);
        self.reset_navigation();
    }

    pub(crate) fn previous(&mut self, current: &str) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        let position = match self.position {
            Some(0) => {
                self.position = None;
                return self.draft.clone();
            }
            Some(position) => position - 1,
            None => {
                self.draft = Some(current.to_owned());
                self.entries.len() - 1
            }
        };
        self.position = Some(position);
        Some(self.entries[position].clone())
    }

    pub(crate) fn next(&mut self) -> Option<String> {
        let Some(position) = self.position else {
            if self.entries.is_empty() {
                return None;
            }
            self.position = Some(0);
            return Some(self.entries[0].clone());
        };
        if position + 1 < self.entries.len() {
            let position = position + 1;
            self.position = Some(position);
            Some(self.entries[position].clone())
        } else {
            self.position = None;
            self.draft.take()
        }
    }

    pub(crate) fn reset_navigation(&mut self) {
        self.position = None;
        self.draft = None;
    }
}
