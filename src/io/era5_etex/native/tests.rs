use super::*;

const SAMPLE: &str = "fixtures/etex/native-mini/era5-19941023-151821-ml.grib";
const TIMES: [i64; 3] = [1500, 1800, 2100];

fn check_message(bytes: Vec<u8>) -> Result<(), DecodeError> {
    let mut file = CodesFile::new_from_memory(bytes, ProductKind::GRIB)?;
    let message = file.ref_message_iter().next()?.expect("fixture message");
    read_record(
        &message,
        1,
        19_941_023,
        &TIMES,
        &mut BTreeSet::new(),
        &mut None,
    )?;
    Ok(())
}

pub(super) fn section_offset(bytes: &[u8], section: u8) -> usize {
    let mut offset = 16;
    while bytes[offset + 4] != section {
        offset += u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
    }
    offset
}

#[test]
fn test_era5_etex_mutated_grib_keys_rejected() {
    let original = std::fs::read(SAMPLE).unwrap()[..MESSAGE_BYTES].to_vec();
    check_message(original.clone()).unwrap();
    for (section, byte, value, key) in [
        (1, 14, 13, "dataDate"),
        (1, 16, 14, "dataTime"),
        (3, 71, 64, "scanningMode"),
        (3, 69, 0, "jDirectionIncrement"),
        (4, 10, 255, "paramId"),
        (4, 27, 138, "level"),
        (4, 6, 21, "NV"),
        (4, 37, 1, "pv SHA-256"),
        (5, 10, 2, "dataRepresentationTemplateNumber"),
        (5, 19, 15, "bitsPerValue"),
        (6, 5, 0, "bitmapPresent"),
    ] {
        let mut changed = original.clone();
        let offset = section_offset(&changed, section);
        changed[offset + byte] = value;
        let error = check_message(changed).unwrap_err();
        let rejected = match error {
            DecodeError::Key { key: actual, .. } => actual == key,
            DecodeError::Record { .. } => matches!(key, "paramId" | "level" | "dataTime"),
            DecodeError::Codec(_) => {
                matches!(key, "dataRepresentationTemplateNumber" | "bitmapPresent")
            }
            _ => false,
        };
        assert!(rejected, "{key}: {error}");
    }
}

#[test]
fn test_era5_etex_missing_duplicate_and_corrupt_frames_rejected() {
    let mut bytes = std::fs::read(SAMPLE).unwrap();
    let decode = |bytes| decode_verified(bytes, 19_941_023, &TIMES, 1500, 782_924_400);
    bytes[0] = b'X';
    assert!(matches!(decode(bytes.clone()), Err(DecodeError::Frame(0))));
    bytes[0] = b'G';
    bytes[7] = 1;
    assert!(matches!(decode(bytes.clone()), Err(DecodeError::Frame(0))));
    bytes[7] = 2;
    let last = bytes.len() - 1;
    bytes[last] = b'X';
    assert!(matches!(decode(bytes.clone()), Err(DecodeError::Frame(_))));

    bytes.truncate(bytes.len() - MESSAGE_BYTES);
    assert!(matches!(decode(bytes.clone()), Err(DecodeError::Coverage)));
    bytes.copy_within(..MESSAGE_BYTES, MESSAGE_BYTES);
    assert!(matches!(decode(bytes), Err(DecodeError::Record { .. })));
}

#[test]
fn test_era5_etex_mismatched_source_units_rejected() {
    let bytes = std::fs::read(SAMPLE).unwrap()[..MESSAGE_BYTES].to_vec();
    let mut file = CodesFile::new_from_memory(bytes, ProductKind::GRIB).unwrap();
    let message = file.ref_message_iter().next().unwrap().unwrap();
    let temperature = PARAMETERS
        .iter()
        .find(|entry| entry.param_id == 130)
        .unwrap();
    validate_parameter(&message, 1, temperature).unwrap();
    let incompatible_units = Parameter {
        units: "m s**-1",
        ..*temperature
    };
    assert!(matches!(
        validate_parameter(&message, 1, &incompatible_units),
        Err(DecodeError::Key { key: "units", .. })
    ));
}
