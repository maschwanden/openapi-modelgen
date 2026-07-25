// This file is @generated — do not edit manually.

use crate::model::*;

#[derive(Debug)]
pub struct ValidationError {
    pub details: Vec<String>,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "validation failed: {}", self.details.join("; "))
    }
}

impl std::error::Error for ValidationError {}

pub trait Validation {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

impl<T: Validation> Validation for Vec<T> {
    fn validate(&self) -> Result<(), ValidationError> {
        let mut errors = Vec::new();
        for (i, item) in self.iter().enumerate() {
            if let Err(e) = item.validate() {
                for detail in e.details {
                    errors.push(format!("[{i}]: {detail}"));
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationError { details: errors })
        }
    }
}

impl Validation for Note {
    fn validate(&self) -> Result<(), ValidationError> {
        let mut errors = Vec::new();
        if self.text.chars().count() < 1 {
            errors.push(format!(
                "text: length {} is less than minimum 1",
                self.text.chars().count()
            ));
        }
        if let Some(val) = &self.tag
            && (*val).chars().count() < 1
        {
            errors.push(format!(
                "tag: length {} is less than minimum 1",
                (*val).chars().count()
            ));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationError { details: errors })
        }
    }
}

impl Validation for NewNote {
    fn validate(&self) -> Result<(), ValidationError> {
        let mut errors = Vec::new();
        if self.text.chars().count() < 1 {
            errors.push(format!(
                "text: length {} is less than minimum 1",
                self.text.chars().count()
            ));
        }
        if self.text.chars().count() > 280 {
            errors.push(format!(
                "text: length {} exceeds maximum 280",
                self.text.chars().count()
            ));
        }
        if let Some(val) = &self.tag
            && (*val).chars().count() < 1
        {
            errors.push(format!(
                "tag: length {} is less than minimum 1",
                (*val).chars().count()
            ));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationError { details: errors })
        }
    }
}

impl Validation for ListNotesQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        let mut errors = Vec::new();
        if self.limit < 1 {
            errors.push(format!(
                "limit: value {} is less than minimum 1",
                self.limit
            ));
        }
        if self.limit > 100 {
            errors.push(format!(
                "limit: value {} exceeds maximum 100",
                self.limit
            ));
        }
        if let Some(val) = &self.tag
            && (*val).chars().count() < 1
        {
            errors.push(format!(
                "tag: length {} is less than minimum 1",
                (*val).chars().count()
            ));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationError { details: errors })
        }
    }
}
