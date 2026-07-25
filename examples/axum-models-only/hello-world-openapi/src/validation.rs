// This file is @generated — do not edit manually.

use regex::Regex;
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
                        errors.push("tags: array contains duplicate items".to_string());
                        break;
                    }
                }
            }
        }
        if let Some(val) = &self.attachment
            && let Err(nested) = (*val).validate()
        {
            errors.extend(nested.details.into_iter().map(|e| format!("attachment.{e}")));
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
                        errors.push("tags: array contains duplicate items".to_string());
                        break;
                    }
                }
            }
        }
        if let Some(val) = &self.delivery
            && let Err(nested) = (*val).validate()
        {
            errors.extend(nested.details.into_iter().map(|e| format!("delivery.{e}")));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationError { details: errors })
        }
    }
}

impl Validation for DeliveryChannel {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::EmailDelivery(inner) => inner.validate(),
            Self::SmsDelivery(inner) => inner.validate(),
        }
    }
}

impl Validation for EmailDelivery {
    fn validate(&self) -> Result<(), ValidationError> {
        let mut errors = Vec::new();
        if self.address.chars().count() < 3 {
            errors.push(format!(
                "address: length {} is less than minimum 3",
                self.address.chars().count()
            ));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationError { details: errors })
        }
    }
}

impl Validation for SmsDelivery {
    fn validate(&self) -> Result<(), ValidationError> {
        let mut errors = Vec::new();
        if !Regex::new("^\\+[1-9]\\d{1,14}$").unwrap().is_match(&self.phone_number) {
            errors.push(format!(
                "phone_number: value '{}' does not match pattern '^\\+[1-9]\\d{{1,14}}$'",
                self.phone_number
            ));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationError { details: errors })
        }
    }
}

impl Validation for Attachment {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::ImageAttachment(inner) => inner.validate(),
            Self::LinkAttachment(inner) => inner.validate(),
        }
    }
}

impl Validation for ImageAttachment {}

impl Validation for LinkAttachment {}

impl Validation for ListGreetingsQuery {
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
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationError { details: errors })
        }
    }
}

impl Validation for GreetingLanguage {}
