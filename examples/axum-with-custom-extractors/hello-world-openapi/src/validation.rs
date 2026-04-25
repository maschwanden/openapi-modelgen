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

impl Validation for Greeting {
    fn validate(&self) -> Result<(), ValidationError> {
        let mut errors = Vec::new();
        if self.message.chars().count() < 1 {
            errors.push(format!(
                "message: length {} is less than minimum 1",
                self.message.chars().count()
            ));
        }
        if self.message.chars().count() > 280 {
            errors.push(format!(
                "message: length {} exceeds maximum 280",
                self.message.chars().count()
            ));
        }
        if let Some(val) = &self.tags {
            if (*val).len() > 10 {
                errors.push(format!(
                    "tags: array length {} exceeds maximum 10",
                    (*val).len()
                ));
            }
            {
                let mut seen = std::collections::HashSet::new();
                for item in (*val).iter() {
                    if !seen.insert(item) {
                        errors.push(format!("tags: array contains duplicate items"));
                        break;
                    }
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

impl Validation for CreateGreetingRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        let mut errors = Vec::new();
        if self.message.chars().count() < 1 {
            errors.push(format!(
                "message: length {} is less than minimum 1",
                self.message.chars().count()
            ));
        }
        if self.message.chars().count() > 280 {
            errors.push(format!(
                "message: length {} exceeds maximum 280",
                self.message.chars().count()
            ));
        }
        if let Some(val) = &self.tags {
            if (*val).len() > 10 {
                errors.push(format!(
                    "tags: array length {} exceeds maximum 10",
                    (*val).len()
                ));
            }
            {
                let mut seen = std::collections::HashSet::new();
                for item in (*val).iter() {
                    if !seen.insert(item) {
                        errors.push(format!("tags: array contains duplicate items"));
                        break;
                    }
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

impl Validation for ListGreetingsQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        let mut errors = Vec::new();
        if let Some(val) = &self.limit {
            if (*val) < 1 {
                errors.push(format!(
                    "limit: value {} is less than minimum 1",
                    (*val)
                ));
            }
            if (*val) > 100 {
                errors.push(format!(
                    "limit: value {} exceeds maximum 100",
                    (*val)
                ));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationError { details: errors })
        }
    }
}
