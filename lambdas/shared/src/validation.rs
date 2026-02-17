use crate::errors::MnemogramError;
use serde_json::Value;

/// Input validation helper functions
pub struct Validator;

impl Validator {
    /// Validate required string fields
    pub fn required_string(
        value: Option<&str>,
        field_name: &str,
    ) -> Result<String, MnemogramError> {
        match value {
            Some(v) if !v.trim().is_empty() => Ok(v.trim().to_string()),
            _ => Err(MnemogramError::ValidationError(format!(
                "Field '{}' is required and cannot be empty",
                field_name
            ))),
        }
    }

    /// Validate string length
    pub fn string_length(
        value: &str,
        field_name: &str,
        min: Option<usize>,
        max: Option<usize>,
    ) -> Result<(), MnemogramError> {
        let len = value.len();

        if let Some(min_len) = min {
            if len < min_len {
                return Err(MnemogramError::ValidationError(format!(
                    "Field '{}' must be at least {} characters long",
                    field_name, min_len
                )));
            }
        }

        if let Some(max_len) = max {
            if len > max_len {
                return Err(MnemogramError::ValidationError(format!(
                    "Field '{}' cannot exceed {} characters",
                    field_name, max_len
                )));
            }
        }

        Ok(())
    }

    /// Validate email format
    pub fn email(value: &str, field_name: &str) -> Result<(), MnemogramError> {
        if !value.contains('@') || !value.contains('.') || value.len() < 5 {
            return Err(MnemogramError::ValidationError(format!(
                "Field '{}' must be a valid email address",
                field_name
            )));
        }
        Ok(())
    }

    /// Validate UUID format
    pub fn uuid(value: &str, field_name: &str) -> Result<(), MnemogramError> {
        if value.len() != 36 || value.chars().filter(|&c| c == '-').count() != 4 {
            return Err(MnemogramError::ValidationError(format!(
                "Field '{}' must be a valid UUID",
                field_name
            )));
        }
        Ok(())
    }

    /// Validate positive integer
    pub fn positive_integer(value: i64, field_name: &str) -> Result<(), MnemogramError> {
        if value <= 0 {
            return Err(MnemogramError::ValidationError(format!(
                "Field '{}' must be a positive integer",
                field_name
            )));
        }
        Ok(())
    }

    /// Validate number within range
    pub fn number_range(
        value: i64,
        field_name: &str,
        min: Option<i64>,
        max: Option<i64>,
    ) -> Result<(), MnemogramError> {
        if let Some(min_val) = min {
            if value < min_val {
                return Err(MnemogramError::ValidationError(format!(
                    "Field '{}' must be at least {}",
                    field_name, min_val
                )));
            }
        }

        if let Some(max_val) = max {
            if value > max_val {
                return Err(MnemogramError::ValidationError(format!(
                    "Field '{}' cannot exceed {}",
                    field_name, max_val
                )));
            }
        }

        Ok(())
    }

    /// Validate file size
    pub fn file_size(size_bytes: u64, field_name: &str, max_mb: u64) -> Result<(), MnemogramError> {
        let max_bytes = max_mb * 1024 * 1024;
        if size_bytes > max_bytes {
            return Err(MnemogramError::ValidationError(format!(
                "File '{}' cannot exceed {}MB (current size: {}MB)",
                field_name,
                max_mb,
                size_bytes / (1024 * 1024)
            )));
        }
        Ok(())
    }

    /// Validate allowed file types
    pub fn file_type(filename: &str, allowed_extensions: &[&str]) -> Result<(), MnemogramError> {
        let extension = filename.rsplit('.').next().unwrap_or("").to_lowercase();

        if !allowed_extensions.iter().any(|&ext| ext == extension) {
            return Err(MnemogramError::ValidationError(format!(
                "File type '{}' not allowed. Allowed types: {}",
                extension,
                allowed_extensions.join(", ")
            )));
        }
        Ok(())
    }

    /// Validate JSON structure
    pub fn json_structure(value: &Value, required_fields: &[&str]) -> Result<(), MnemogramError> {
        let obj = value.as_object().ok_or_else(|| {
            MnemogramError::ValidationError("Request body must be a JSON object".to_string())
        })?;

        for &field in required_fields {
            if !obj.contains_key(field) {
                return Err(MnemogramError::ValidationError(format!(
                    "Required field '{}' is missing",
                    field
                )));
            }
        }

        Ok(())
    }

    /// Validate memory name (alphanumeric, hyphens, underscores only)
    pub fn memory_name(name: &str) -> Result<(), MnemogramError> {
        if name.is_empty() {
            return Err(MnemogramError::ValidationError(
                "Memory name cannot be empty".to_string(),
            ));
        }

        if name.len() > 100 {
            return Err(MnemogramError::ValidationError(
                "Memory name cannot exceed 100 characters".to_string(),
            ));
        }

        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == ' ')
        {
            return Err(MnemogramError::ValidationError(
                "Memory name can only contain letters, numbers, spaces, hyphens, and underscores"
                    .to_string(),
            ));
        }

        Ok(())
    }

    /// Validate search query
    pub fn search_query(query: &str) -> Result<(), MnemogramError> {
        if query.trim().is_empty() {
            return Err(MnemogramError::ValidationError(
                "Search query cannot be empty".to_string(),
            ));
        }

        if query.len() > 1000 {
            return Err(MnemogramError::ValidationError(
                "Search query cannot exceed 1000 characters".to_string(),
            ));
        }

        Ok(())
    }

    /// Validate pagination parameters
    pub fn pagination(
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<(i64, i64), MnemogramError> {
        let limit = limit.unwrap_or(10);
        let offset = offset.unwrap_or(0);

        if !(1..=100).contains(&limit) {
            return Err(MnemogramError::ValidationError(
                "Limit must be between 1 and 100".to_string(),
            ));
        }

        if offset < 0 {
            return Err(MnemogramError::ValidationError(
                "Offset cannot be negative".to_string(),
            ));
        }

        Ok((limit, offset))
    }
}

/// Helper struct to accumulate validation errors
pub struct ValidationErrors {
    errors: Vec<String>,
}

impl ValidationErrors {
    pub fn new() -> Self {
        Self { errors: Vec::new() }
    }

    pub fn add(&mut self, error: &str) {
        self.errors.push(error.to_string());
    }

    pub fn add_result<T>(&mut self, result: Result<T, MnemogramError>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(MnemogramError::ValidationError(msg)) => {
                self.errors.push(msg);
                None
            }
            Err(other) => {
                self.errors.push(other.to_string());
                None
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn into_result(self) -> Result<(), MnemogramError> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(MnemogramError::ValidationError(self.errors.join("; ")))
        }
    }

    pub fn errors(&self) -> &[String] {
        &self.errors
    }
}

impl Default for ValidationErrors {
    fn default() -> Self {
        Self::new()
    }
}
