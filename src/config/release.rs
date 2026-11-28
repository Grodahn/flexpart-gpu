//! Existing RELEASES input adapter and species binding.

use std::path::Path;

use sha2::{Digest, Sha256};

use super::parsing::{
    parse_release_assignments, read_text_file, release_namelist_sections, strip_comments, ConfigMap,
};
use super::{ConfigError, SpeciesConfig};
use crate::release::{
    invalid_release, validate_release_bindings, validate_release_species, ReleaseConversion,
    ReleaseGeometry, ReleaseHeight, ReleaseHeightRef, ReleaseHorizontalRef, ReleaseInventory,
    ReleaseProfile, ReleaseQuantity, ReleaseSource, ReleaseSpec, ReleaseSpecFields, ReleaseTime,
    GRAMS_PER_KILOGRAM,
};

#[derive(Debug, Clone, PartialEq)]
enum SpeciesSelection {
    Names(Vec<String>),
    FlexpartNumbers(Vec<u16>),
    RunOrder,
    SoleConfiguredSpecies,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMassUnit {
    Kilograms,
    Grams,
}

/// A typed RELEASES declaration awaiting binding to the validated run species table.
#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseConfig {
    id: String,
    profile: ReleaseProfile,
    geometry: ReleaseGeometry,
    height: ReleaseHeight,
    mass_values: Vec<f64>,
    mass_unit: InputMassUnit,
    species: SpeciesSelection,
    particle_count: u64,
    sampling_seed: Option<u64>,
    source: ReleaseSource,
}

impl ReleaseConfig {
    /// Read all declarations from the existing RELEASES file path.
    ///
    /// # Errors
    /// Returns a configuration error if the file cannot be read or parsed.
    pub fn load_many_from_file(path: &Path) -> Result<Vec<Self>, ConfigError> {
        Self::parse_many(&read_text_file(path)?, path)
    }

    /// Parse native FLEXPART 11.1 or project RELEASES without retaining untyped assignments.
    ///
    /// # Errors
    /// Returns a configuration error if the syntax, source identity or mandatory fields are invalid.
    pub fn parse_many(input: &str, source: &Path) -> Result<Vec<Self>, ConfigError> {
        let path = source
            .to_str()
            .filter(|path| !path.trim().is_empty())
            .ok_or_else(|| invalid_release("RELEASES requires a non-empty UTF-8 source path"))?;
        let sha256 = format!("{:x}", Sha256::digest(input.as_bytes()));
        let cleaned = strip_comments(input);
        let error = |message: String| ConfigError::Parse {
            context: format!("RELEASES ({path})"),
            message,
        };
        let mut sections = if cleaned.trim_start().starts_with('&') {
            release_namelist_sections(&cleaned).map_err(error)?
        } else {
            cleaned
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| ("release".to_owned(), line.to_owned()))
                .collect()
        };
        let native = if sections
            .first()
            .is_some_and(|(name, _)| name == "releases_ctrl")
        {
            let (_, control) = sections.remove(0);
            Some(parse_native_control(
                parse_release_assignments(&control).map_err(error)?,
            )?)
        } else {
            None
        };
        if sections.is_empty() {
            return Err(error("no release entries found".to_string()));
        }
        sections
            .into_iter()
            .enumerate()
            .map(|(index, (name, body))| {
                if name != "release" {
                    return Err(error(format!("unexpected namelist &{name}")));
                }
                let record = u32::try_from(index + 1)
                    .map_err(|_| error("too many release records".to_owned()))?;
                let fields = Fields(parse_release_assignments(&body).map_err(error)?);
                let provenance = if native.is_some() {
                    ReleaseSource::Flexpart11_1ReleasesV1 {
                        path: path.to_owned(),
                        sha256: sha256.clone(),
                        record,
                    }
                } else {
                    ReleaseSource::ProjectReleasesV1 {
                        path: path.to_owned(),
                        sha256: sha256.clone(),
                        record,
                    }
                };
                Self::from_fields(fields, native.as_deref(), record, provenance)
                    .map_err(|cause| error(format!("record {record}: {cause}")))
            })
            .collect()
    }

    /// Resolve all species against the run table and validate the complete ordered release set.
    ///
    /// For native namelists the caller must establish forward mass mode (`LDIRECT=1`, `IND_SOURCE=1`).
    /// The aggregate configuration loader checks these COMMAND switches before calling this API.
    ///
    /// # Errors
    /// Returns a configuration error if a release or its species binding is invalid.
    pub fn normalize_forward_mass(
        input: &[Self],
        species: &[SpeciesConfig],
    ) -> Result<Vec<ReleaseSpec>, ConfigError> {
        validate_release_species(species)?;
        let specs = input
            .iter()
            .map(|config| config.normalize(species))
            .collect::<Result<Vec<_>, _>>()?;
        validate_release_bindings(&specs, species)?;
        Ok(specs)
    }

    pub(super) fn uses_native_release_mode(&self) -> bool {
        matches!(self.source, ReleaseSource::Flexpart11_1ReleasesV1 { .. })
    }

    fn normalize(&self, species: &[SpeciesConfig]) -> Result<ReleaseSpec, ConfigError> {
        let error = |message| invalid_release(format!("release `{}`: {message}", self.id));
        let slots = self.resolve_slots(species)?;
        if slots.len() != self.mass_values.len() {
            return Err(error("species and mass counts differ".to_owned()));
        }

        let inventories = slots
            .into_iter()
            .zip(&self.mass_values)
            .map(|(slot, &amount)| {
                let (kg, conversion) = if self.mass_unit == InputMassUnit::Grams {
                    let kg = amount / GRAMS_PER_KILOGRAM;
                    if amount != 0.0 && kg == 0.0 {
                        return Err(error("release mass underflows kilograms".to_owned()));
                    }
                    (
                        kg,
                        Some(ReleaseConversion::DeclaredGramsToKgV1 { input_g: amount }),
                    )
                } else {
                    (amount, None)
                };
                Ok(ReleaseInventory {
                    species_id: species[slot].name.clone(),
                    mass_slot: u8::try_from(slot)
                        .map_err(|_| error("mass slot exceeds production range".to_owned()))?,
                    quantity: ReleaseQuantity::Mass { kg },
                    conversion,
                })
            })
            .collect::<Result<Vec<_>, ConfigError>>()?;
        ReleaseSpec::new(ReleaseSpecFields {
            id: self.id.clone(),
            profile: self.profile,
            horizontal_ref: ReleaseHorizontalRef::GeographicWgs84Degrees,
            geometry: self.geometry,
            height: self.height,
            inventories,
            particle_count: self.particle_count,
            sampling_seed: self.sampling_seed,
            source: self.source.clone(),
        })
    }

    fn resolve_slots(&self, species: &[SpeciesConfig]) -> Result<Vec<usize>, ConfigError> {
        match &self.species {
            SpeciesSelection::Names(names) => names
                .iter()
                .map(|name| {
                    species
                        .iter()
                        .position(|item| item.name.eq_ignore_ascii_case(name))
                        .ok_or_else(|| {
                            invalid_release(format!(
                                "release `{}`: unknown species `{name}`",
                                self.id
                            ))
                        })
                })
                .collect(),
            SpeciesSelection::FlexpartNumbers(numbers) => numbers
                .iter()
                .map(|number| resolve_flexpart_species(*number, species))
                .collect(),
            SpeciesSelection::RunOrder if self.mass_values.len() == species.len() => {
                Ok((0..species.len()).collect())
            }
            SpeciesSelection::SoleConfiguredSpecies if species.len() == 1 => Ok(vec![0]),
            _ => Err(invalid_release(format!(
                "release `{}`: declare species or provide a complete run-ordered mass list",
                self.id
            ))),
        }
    }

    fn from_fields(
        mut fields: Fields,
        native: Option<&[u16]>,
        record: u32,
        source: ReleaseSource,
    ) -> Result<Self, ConfigError> {
        let id = fields
            .take(&["name", "release_name", "id", "comment"])?
            .unwrap_or_else(|| format!("release_{record}"));
        let (
            profile,
            geometry,
            height,
            mass_values,
            mass_unit,
            species,
            particle_count,
            sampling_seed,
        ) = if let Some(numbers) = native {
            let start = read_native_time(&mut fields, "idate1", "itime1")?;
            let end = read_native_time(&mut fields, "idate2", "itime2")?;
            let profile = profile(start, end, None)?;
            let geometry = read_bounds(&mut fields, &["lon1"], &["lon2"], &["lat1"], &["lat2"])?;
            let height = read_height(&mut fields, true)?;
            let masses = parse_list(&fields.required(&["mass"])?, parse_float)?;
            let count = fields.parse(&["parts"], parse_count)?;
            (
                profile,
                geometry,
                height,
                masses,
                InputMassUnit::Kilograms,
                SpeciesSelection::FlexpartNumbers(numbers.to_vec()),
                count,
                None,
            )
        } else {
            let profile = read_project_profile(&mut fields)?;
            let geometry = read_project_geometry(&mut fields)?;
            let height = read_height(&mut fields, false)?;
            let (masses, mass_unit, species) = read_project_mass(&mut fields)?;
            let count = fields.parse(&["particle_count", "particles", "npart"], parse_count)?;
            let seed = fields
                .take(&["sampling_seed"])?
                .map(|value| parse_count(&value))
                .transpose()?;
            (
                profile, geometry, height, masses, mass_unit, species, count, seed,
            )
        };
        fields.finish()?;
        Ok(Self {
            id,
            profile,
            geometry,
            height,
            mass_values,
            mass_unit,
            species,
            particle_count,
            sampling_seed,
            source,
        })
    }
}

/// Resolve a FLEXPART `SPECNUM_REL` identity against #10 source filenames, independently of slot order.
pub(crate) fn resolve_flexpart_species(
    number: u16,
    species: &[SpeciesConfig],
) -> Result<usize, ConfigError> {
    let expected = format!("SPECIES_{number:03}");
    let mut matches = species.iter().enumerate().filter(|(_, item)| {
        item.source_file
            .as_deref()
            .and_then(|file| Path::new(file).file_name())
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|name| name.eq_ignore_ascii_case(&expected))
    });
    let first = matches.next().map(|(index, _)| index);
    if matches.next().is_some() || first.is_none() {
        return Err(invalid_release(format!(
            "expected one configured species source file named {expected}"
        )));
    }
    first.ok_or_else(|| invalid_release(format!("unknown species {expected}")))
}

fn parse_native_control(map: ConfigMap) -> Result<Vec<u16>, ConfigError> {
    let mut fields = Fields(map);
    let count = fields.parse(&["nspec"], parse_count)?;
    let numbers = parse_list(&fields.required(&["specnum_rel"])?, |text| {
        text.parse::<u16>()
            .ok()
            .filter(|number| (1..=999).contains(number))
            .ok_or_else(|| invalid_release(format!("invalid SPECNUM_REL: {text}")))
    })?;
    fields.finish()?;
    if count != numbers.len() as u64
        || numbers.len() > crate::particles::MAX_SPECIES
        || numbers
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != numbers.len()
    {
        return Err(invalid_release(
            "NSPEC/SPECNUM_REL must select one to four distinct species",
        ));
    }
    Ok(numbers)
}

fn read_native_time(
    fields: &mut Fields,
    date: &str,
    time: &str,
) -> Result<ReleaseTime, ConfigError> {
    let day = fields.required(&[date])?;
    let clock = fields.required(&[time])?;
    if day.len() != 8 || clock.len() > 6 || !clock.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_release(
            "expected IDATE=YYYYMMDD and numeric ITIME=HHMMSS in UTC",
        ));
    }
    ReleaseTime::parse_utc(&format!("{day}{clock:0>6}"))
}

fn project_time(value: &str) -> Result<ReleaseTime, ConfigError> {
    let date =
        if matches!(value.len(), 8 | 10 | 12) && value.bytes().all(|byte| byte.is_ascii_digit()) {
            format!("{value:0<14}")
        } else {
            value.to_owned()
        };
    ReleaseTime::parse_utc(&date)
}

fn read_project_profile(fields: &mut Fields) -> Result<ReleaseProfile, ConfigError> {
    let start = fields.parse(&["start_time", "start"], project_time)?;
    let end = fields.parse(&["end_time", "end"], project_time)?;
    let kind = fields.take(&["profile"])?;
    profile(start, end, kind.as_deref())
}

fn profile(
    start: ReleaseTime,
    end: ReleaseTime,
    kind: Option<&str>,
) -> Result<ReleaseProfile, ConfigError> {
    match (kind, start.cmp(&end)) {
        (None | Some("instant"), std::cmp::Ordering::Equal) => {
            Ok(ReleaseProfile::Instant { at: start })
        }
        (None | Some("uniform_window"), std::cmp::Ordering::Less) => {
            Ok(ReleaseProfile::UniformWindow { start, end })
        }
        _ => Err(invalid_release(
            "unsupported release profile or inconsistent time interval",
        )),
    }
}

fn read_project_geometry(fields: &mut Fields) -> Result<ReleaseGeometry, ConfigError> {
    let lon = fields.take(&["lon", "longitude"])?;
    let lat = fields.take(&["lat", "latitude"])?;
    let has_bounds = fields.0.keys().any(|key| {
        matches!(
            key.as_str(),
            "lon_min"
                | "xpoint1"
                | "xlon1"
                | "lon_max"
                | "xpoint2"
                | "xlon2"
                | "lat_min"
                | "ypoint1"
                | "ylat1"
                | "lat_max"
                | "ypoint2"
                | "ylat2"
        )
    });
    match (lon, lat, has_bounds) {
        (Some(lon), Some(lat), false) => Ok(ReleaseGeometry::Point {
            lon_deg: parse_float(&lon)?,
            lat_deg: parse_float(&lat)?,
        }),
        (None, None, true) => read_bounds(
            fields,
            &["lon_min", "xpoint1", "xlon1"],
            &["lon_max", "xpoint2", "xlon2"],
            &["lat_min", "ypoint1", "ylat1"],
            &["lat_max", "ypoint2", "ylat2"],
        ),
        _ => Err(invalid_release(
            "declare either both point coordinates or all four area bounds",
        )),
    }
}

fn read_bounds(
    fields: &mut Fields,
    west: &[&str],
    east: &[&str],
    south: &[&str],
    north: &[&str],
) -> Result<ReleaseGeometry, ConfigError> {
    Ok(ReleaseGeometry::from_bounds(
        fields.parse(west, parse_float)?,
        fields.parse(east, parse_float)?,
        fields.parse(south, parse_float)?,
        fields.parse(north, parse_float)?,
    ))
}

fn read_height(fields: &mut Fields, native: bool) -> Result<ReleaseHeight, ConfigError> {
    let reference = if native {
        fields.required(&["zkind"])?
    } else {
        fields.required(&["vertical_ref", "height_ref", "zkind"])?
    };
    let datum = match reference.to_ascii_lowercase().as_str() {
        "1" => ReleaseHeightRef::Agl,
        "2" => ReleaseHeightRef::Asl,
        "agl" if !native => ReleaseHeightRef::Agl,
        "asl" if !native => ReleaseHeightRef::Asl,
        _ => {
            return Err(invalid_release(format!(
                "unsupported vertical reference `{reference}`"
            )))
        }
    };
    let lower = fields.parse(
        if native {
            &["z1"]
        } else {
            &["z_min", "z1", "z_bottom"]
        },
        parse_float,
    )?;
    let upper = fields.parse(
        if native {
            &["z2"]
        } else {
            &["z_max", "z2", "z_top"]
        },
        parse_float,
    )?;
    Ok(ReleaseHeight::from_bounds(datum, lower, upper))
}

fn read_project_mass(
    fields: &mut Fields,
) -> Result<(Vec<f64>, InputMassUnit, SpeciesSelection), ConfigError> {
    let explicit_kg = fields.0.contains_key("mass_kg");
    let scalar = fields.take(&["mass_kg", "mass", "xmass"])?;
    let vector = fields.take(&["mass_species", "species_mass", "species_masses"])?;
    let declared_unit = fields.take(&["mass_unit", "mass_units"])?;
    let unit = match declared_unit
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None | Some("kg") => InputMassUnit::Kilograms,
        Some("g") if !explicit_kg => InputMassUnit::Grams,
        _ => {
            return Err(invalid_release(
                "unknown or conflicting mass unit; MASS_KG always means kg",
            ))
        }
    };
    let species = fields
        .take(&["species", "species_names"])?
        .map(|value| parse_species_names(&value).map(SpeciesSelection::Names))
        .transpose()?;
    match (scalar, vector) {
        (Some(value), None) => Ok((
            vec![parse_float(&value)?],
            unit,
            species.unwrap_or(SpeciesSelection::SoleConfiguredSpecies),
        )),
        (None, Some(value)) => Ok((
            parse_list(&value, parse_float)?,
            unit,
            species.unwrap_or(SpeciesSelection::RunOrder),
        )),
        _ => Err(invalid_release(
            "declare exactly one mass inventory; activity input is unsupported by RELEASES",
        )),
    }
}

fn parse_species_names(value: &str) -> Result<Vec<String>, ConfigError> {
    value
        .split(',')
        .map(|name| {
            let name = name.trim();
            if name.is_empty() {
                return Err(invalid_release("empty release species name"));
            }
            Ok(name.to_owned())
        })
        .collect()
}

fn parse_list<T>(
    value: &str,
    parse: impl Fn(&str) -> Result<T, ConfigError>,
) -> Result<Vec<T>, ConfigError> {
    let segments = value.split(',');
    if segments.clone().any(|segment| segment.trim().is_empty()) {
        return Err(invalid_release("empty release list entry"));
    }
    segments
        .flat_map(str::split_whitespace)
        .map(parse)
        .collect()
}

fn parse_float(value: &str) -> Result<f64, ConfigError> {
    let text = value.replace(['d', 'D'], "e");
    let number = text
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
        .ok_or_else(|| invalid_release(format!("expected finite number: {value}")))?;
    if number == 0.0
        && text
            .bytes()
            .take_while(|byte| !matches!(byte, b'e' | b'E'))
            .any(|byte| (b'1'..=b'9').contains(&byte))
    {
        return Err(invalid_release(format!("number underflows f64: {value}")));
    }
    Ok(number)
}

fn parse_count(value: &str) -> Result<u64, ConfigError> {
    value
        .parse::<u64>()
        .map_err(|_| invalid_release(format!("expected unsigned integer: {value}")))
}

struct Fields(ConfigMap);

impl Fields {
    fn take(&mut self, aliases: &[&str]) -> Result<Option<String>, ConfigError> {
        let mut values = aliases.iter().filter_map(|key| self.0.remove(*key));
        let value = values.next();
        if values.next().is_some() {
            return Err(invalid_release(format!(
                "conflicting aliases for {}",
                aliases[0]
            )));
        }
        Ok(value)
    }

    fn required(&mut self, aliases: &[&str]) -> Result<String, ConfigError> {
        self.take(aliases)?
            .ok_or_else(|| invalid_release(format!("missing {}", aliases[0])))
    }

    fn parse<T>(
        &mut self,
        aliases: &[&str],
        parse: impl FnOnce(&str) -> Result<T, ConfigError>,
    ) -> Result<T, ConfigError> {
        parse(&self.required(aliases)?)
    }

    fn finish(self) -> Result<(), ConfigError> {
        if let Some((key, _)) = self.0.first_key_value() {
            return Err(invalid_release(format!(
                "unsupported RELEASES field: {key}"
            )));
        }
        Ok(())
    }
}
