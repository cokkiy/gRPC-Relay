use relay_proto::relay::v1::ErrorCode;

const MAX_PAYLOAD_BYTES: usize = 10 * 1024 * 1024; // 10MB
const MAX_ID_LENGTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    pub code: ErrorCode,
    pub message: String,
}

pub fn validate_controller_message(
    controller_id: &str,
    target_device_id: &str,
    method_name: &str,
    encrypted_payload: &[u8],
    sequence_number: i64,
) -> Result<(), ValidationError> {
    validate_controller_id(controller_id)?;
    validate_device_id(target_device_id)?;
    validate_method_name(method_name)?;
    validate_payload_size(encrypted_payload.len())?;
    validate_sequence_number(sequence_number)?;
    Ok(())
}

pub fn validate_controller_id(id: &str) -> Result<(), ValidationError> {
    if id.trim().is_empty() {
        return Err(ValidationError {
            code: ErrorCode::Unauthorized,
            message: "controller_id must not be empty".into(),
        });
    }
    if id.len() > MAX_ID_LENGTH {
        return Err(ValidationError {
            code: ErrorCode::Unauthorized,
            message: format!("controller_id too long (max {MAX_ID_LENGTH} chars)"),
        });
    }
    Ok(())
}

pub fn validate_device_id(id: &str) -> Result<(), ValidationError> {
    if id.trim().is_empty() {
        return Err(ValidationError {
            code: ErrorCode::DeviceNotFound,
            message: "target_device_id must not be empty".into(),
        });
    }
    if id.len() > MAX_ID_LENGTH {
        return Err(ValidationError {
            code: ErrorCode::DeviceNotFound,
            message: format!("target_device_id too long (max {MAX_ID_LENGTH} chars)"),
        });
    }
    Ok(())
}

pub fn validate_method_name(name: &str) -> Result<(), ValidationError> {
    if name.trim().is_empty() {
        return Err(ValidationError {
            code: ErrorCode::InternalError,
            message: "method_name must not be empty".into(),
        });
    }
    // Allow ASCII alphanumeric, underscores, dots, slashes (gRPC fully-qualified names)
    let allowed = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '/');
    if !allowed {
        return Err(ValidationError {
            code: ErrorCode::InternalError,
            message: "method_name contains invalid characters".into(),
        });
    }
    Ok(())
}

pub fn validate_payload_size(size: usize) -> Result<(), ValidationError> {
    if size > MAX_PAYLOAD_BYTES {
        return Err(ValidationError {
            code: ErrorCode::InternalError,
            message: format!("payload exceeds maximum size of {MAX_PAYLOAD_BYTES} bytes"),
        });
    }
    Ok(())
}

pub fn validate_sequence_number(seq: i64) -> Result<(), ValidationError> {
    if seq <= 0 {
        return Err(ValidationError {
            code: ErrorCode::InternalError,
            message: "sequence_number must be positive".into(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_controller_message_passes() {
        assert!(validate_controller_message(
            "ctrl-123",
            "dev-456",
            "ExecuteCommand",
            &[0u8; 100],
            1
        )
        .is_ok());
    }

    #[test]
    fn empty_controller_id_fails() {
        let err = validate_controller_id("").unwrap_err();
        assert_eq!(err.code, ErrorCode::Unauthorized);
    }

    #[test]
    fn empty_device_id_fails() {
        let err = validate_device_id("").unwrap_err();
        assert_eq!(err.code, ErrorCode::DeviceNotFound);
    }

    #[test]
    fn empty_method_name_fails() {
        let err = validate_method_name("").unwrap_err();
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn oversized_payload_fails() {
        let err = validate_payload_size(MAX_PAYLOAD_BYTES + 1).unwrap_err();
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn zero_sequence_number_fails() {
        let err = validate_sequence_number(0).unwrap_err();
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn negative_sequence_number_fails() {
        let err = validate_sequence_number(-1).unwrap_err();
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn long_ids_fail() {
        let long = "a".repeat(65);
        assert!(validate_controller_id(&long).is_err());
        assert!(validate_device_id(&long).is_err());
    }

    #[test]
    fn method_name_allows_valid_chars() {
        assert!(validate_method_name("my.package.MyService/DoSomething").is_ok());
        assert!(validate_method_name("DoSomething").is_ok());
        assert!(validate_method_name("do_something").is_ok());
    }

    #[test]
    fn method_name_rejects_invalid_chars() {
        assert!(validate_method_name("hello world").is_err());
        assert!(validate_method_name("method!").is_err());
    }
}
