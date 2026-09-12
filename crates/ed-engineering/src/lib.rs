//! Local engineering blueprint database + inventory-gap checking.
//!
//! The blueprint data (`data/blueprints.json`) is vendored from the
//! MIT-licensed EDEngineer project (<https://github.com/msarilar/EDEngineer>),
//! not hand-authored -- material costs are exactly the kind of thing that
//! must never be guessed from memory, since a wrong number here quietly
//! sends the player off to farm the wrong quantity.

pub mod journal;
pub mod sources;
pub mod trader;
pub mod unlocks;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const BLUEPRINTS_JSON: &str = include_str!("../data/blueprints.json");
/// Synthesis recipes (`data/synthesis.json`), machine-extracted from the
/// community wiki's recipe tables (source and fetch date in the file).
/// A JSON library on purpose (maintainer, 2026-09-07): knowledge lands here as
/// data, not code, so the next recipe is a file edit.
const SYNTHESIS_JSON: &str = include_str!("../data/synthesis.json");

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Ingredient {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Size")]
    pub quantity: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Effect {
    #[serde(rename = "Effect")]
    pub effect: String,
    #[serde(rename = "Property")]
    pub property: String,
    #[serde(rename = "IsGood")]
    pub is_good: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Blueprint {
    #[serde(rename = "Type")]
    pub module_type: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Engineers", default)]
    pub engineers: Vec<String>,
    #[serde(rename = "Ingredients", default)]
    pub ingredients: Vec<Ingredient>,
    #[serde(rename = "Effects", default)]
    pub effects: Vec<Effect>,
    #[serde(rename = "Grade", default)]
    pub grade: Option<i64>,
    #[serde(rename = "CoriolisGuid", default)]
    pub coriolis_guid: String,
}

/// One synthesis recipe: a name ("FSD Injection"), what it refills, and
/// its grades with their bonus text and material costs. Material names
/// are the game's display names; `ed_journal::Catalog::by_name` maps them
/// to journal symbols for inventory checks.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SynthesisRecipe {
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub refills: String,
    pub grades: Vec<SynthesisGrade>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SynthesisGrade {
    pub grade: String,
    #[serde(default)]
    pub bonus: Option<String>,
    pub ingredients: Vec<SynthesisIngredient>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SynthesisIngredient {
    pub name: String,
    pub count: i64,
}

#[derive(Debug, Deserialize)]
struct SynthesisFile {
    source: String,
    fetched: String,
    recipes: Vec<SynthesisRecipe>,
}

/// Where a library's rows came from and when.
#[derive(Debug, Clone, Serialize)]
pub struct Provenance {
    pub source: String,
    pub fetched: String,
}

pub struct Catalog {
    blueprints: Vec<Blueprint>,
    synthesis: Vec<SynthesisRecipe>,
    synthesis_provenance: Provenance,
}

#[derive(Debug, Serialize)]
pub struct RequirementLine {
    pub material: String,
    pub need: i64,
    pub have: i64,
}

#[derive(Debug, Serialize)]
pub struct GapReport {
    pub module_type: String,
    pub name: String,
    pub grade: i64,
    pub lines: Vec<RequirementLine>,
    pub fully_met: bool,
}

impl Catalog {
    /// Every distinct (module type, blueprint name) that has at least one
    /// graded row -- the set a proposed-engineering export must cover.
    pub fn graded_pairs(&self) -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = self
            .blueprints
            .iter()
            .filter(|b| b.grade.is_some())
            .map(|b| (b.module_type.clone(), b.name.clone()))
            .collect();
        pairs.sort();
        pairs.dedup();
        pairs
    }

    pub fn load() -> Self {
        let blueprints: Vec<Blueprint> =
            serde_json::from_str(BLUEPRINTS_JSON).expect("bundled blueprints.json must parse");
        let synthesis: SynthesisFile =
            serde_json::from_str(SYNTHESIS_JSON).expect("bundled synthesis.json must parse");
        Catalog {
            blueprints,
            synthesis: synthesis.recipes,
            synthesis_provenance: Provenance {
                source: synthesis.source,
                fetched: synthesis.fetched,
            },
        }
    }

    /// Every synthesis recipe, in the page's order.
    pub fn synthesis_recipes(&self) -> &[SynthesisRecipe] {
        &self.synthesis
    }

    pub fn synthesis_provenance(&self) -> &Provenance {
        &self.synthesis_provenance
    }

    /// A recipe by name, case-insensitively; a unique substring match
    /// ("injection", "heat sink") also lands.
    pub fn find_synthesis(&self, name: &str) -> Option<&SynthesisRecipe> {
        let want = name.trim().to_ascii_lowercase();
        if want.is_empty() {
            return None;
        }
        if let Some(exact) = self
            .synthesis
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(&want))
        {
            return Some(exact);
        }
        let mut hits = self
            .synthesis
            .iter()
            .filter(|r| r.name.to_ascii_lowercase().contains(&want));
        match (hits.next(), hits.next()) {
            (Some(one), None) => Some(one),
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        self.blueprints.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blueprints.is_empty()
    }

    /// All distinct blueprint names for a given module type (e.g. "Frame Shift Drive").
    pub fn blueprint_names_for(&self, module_type: &str) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .blueprints
            .iter()
            .filter(|b| b.module_type.eq_ignore_ascii_case(module_type))
            .map(|b| b.name.as_str())
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// All distinct module types in the database.
    /// Grades a blueprint actually comes in, ascending. Empty for an
    /// experimental effect, which has no grade at all -- offering "grade 5"
    /// for one is how the UI produced an impossible request.
    pub fn grades_for(&self, module_type: &str, name: &str) -> Vec<i64> {
        let mut grades: Vec<i64> = self
            .blueprints
            .iter()
            .filter(|b| {
                b.module_type.eq_ignore_ascii_case(module_type) && b.name.eq_ignore_ascii_case(name)
            })
            .filter_map(|b| b.grade)
            .collect();
        grades.sort_unstable();
        grades.dedup();
        grades
    }

    /// Every material any blueprint asks for, deduplicated.
    pub fn all_ingredient_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .blueprints
            .iter()
            .flat_map(|b| b.ingredients.iter().map(|i| i.name.clone()))
            .collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn module_types(&self) -> Vec<&str> {
        let mut types: Vec<&str> = self
            .blueprints
            .iter()
            .map(|b| b.module_type.as_str())
            .collect();
        types.sort_unstable();
        types.dedup();
        types
    }

    /// Looks up a graded blueprint (grades 1-5, e.g. "Frame Shift Drive" / "Increased FSD Range" / 5).
    pub fn find(&self, module_type: &str, name: &str, grade: i64) -> Option<&Blueprint> {
        self.blueprints.iter().find(|b| {
            b.module_type.eq_ignore_ascii_case(module_type)
                && b.name.eq_ignore_ascii_case(name)
                && b.grade == Some(grade)
        })
    }

    /// Looks up an experimental effect (ungraded, e.g. "Frame Shift Drive" / "Mass Manager") --
    /// about a fifth of the vendored dataset has no Grade field at all, since
    /// experimental effects are a single one-time application, not a 1-5 ladder.
    pub fn find_experimental(&self, module_type: &str, name: &str) -> Option<&Blueprint> {
        self.blueprints.iter().find(|b| {
            b.module_type.eq_ignore_ascii_case(module_type)
                && b.name.eq_ignore_ascii_case(name)
                && b.grade.is_none()
        })
    }

    /// Free-text search across module type + blueprint name, for a fuzzy
    /// "what do I search for X" entry point in the UI.
    pub fn search(&self, text: &str) -> Vec<&Blueprint> {
        let needle = text.to_lowercase();
        self.blueprints
            .iter()
            .filter(|b| {
                b.module_type.to_lowercase().contains(&needle)
                    || b.name.to_lowercase().contains(&needle)
            })
            .collect()
    }

    /// Combined ingredient totals to go from grade 1 up through `target_grade`
    /// of a blueprint (the realistic engineering workflow -- you can't skip
    /// grades). If the player is already partway, pass their current grade
    /// as `from_grade` (0 means "from scratch").
    pub fn cumulative_requirements(
        &self,
        module_type: &str,
        name: &str,
        from_grade: i64,
        target_grade: i64,
    ) -> HashMap<String, i64> {
        let mut totals: HashMap<String, i64> = HashMap::new();
        for grade in (from_grade + 1)..=target_grade {
            if let Some(bp) = self.find(module_type, name, grade) {
                for ing in &bp.ingredients {
                    *totals.entry(ing.name.clone()).or_insert(0) += ing.quantity;
                }
            }
        }
        totals
    }

    /// Diff cumulative requirements against a live inventory (material name,
    /// lowercase or not, -> count you have). Reuses whatever the caller
    /// already resolved via ed-journal's Catalog display names, so names
    /// line up between the two crates' data sources.
    /// Rolls needed at `grade` before the next grade unlocks: `grade` of
    /// them. Measured from the commander's own journal (36 grade
    /// progressions, minimum 1/2/3/4 at grades 1-4; averages ran higher only
    /// because of re-rolls for quality) and consistent with the documented
    /// mechanic. The target grade itself is rolled once.
    pub fn rolls_to_unlock_next(grade: i64) -> i64 {
        grade.max(1)
    }

    /// Requirements from `from_grade` to `target_grade` the way engineering
    /// actually works: every grade below the target is rolled enough times
    /// to unlock the next one; the target grade either N times as well
    /// (`complete_target`: each roll at grade N adds 1/N, measured from the
    /// journal -- 0.2 per grade-5 roll) or once, which merely reaches it.
    /// `gap_report` (minimum) counts one roll per grade, which nobody has
    /// ever achieved past grade 1.
    pub fn cumulative_requirements_realistic(
        &self,
        module_type: &str,
        name: &str,
        from_grade: i64,
        target_grade: i64,
        complete_target: bool,
    ) -> HashMap<String, i64> {
        let mut totals: HashMap<String, i64> = HashMap::new();
        for grade in (from_grade + 1)..=target_grade {
            let rolls = if grade < target_grade || complete_target {
                Self::rolls_to_unlock_next(grade)
            } else {
                1
            };
            if let Some(bp) = self.find(module_type, name, grade) {
                for ing in &bp.ingredients {
                    *totals.entry(ing.name.clone()).or_insert(0) += ing.quantity * rolls;
                }
            }
        }
        totals
    }

    /// `gap_report` with realistic roll counts. `grade` is the target.
    pub fn gap_report_realistic(
        &self,
        module_type: &str,
        name: &str,
        from_grade: i64,
        target_grade: i64,
        complete_target: bool,
        have: &HashMap<String, i64>,
    ) -> GapReport {
        let totals = self.cumulative_requirements_realistic(
            module_type,
            name,
            from_grade,
            target_grade,
            complete_target,
        );
        let have_lower: HashMap<String, i64> =
            have.iter().map(|(k, v)| (k.to_lowercase(), *v)).collect();
        let mut lines: Vec<RequirementLine> = totals
            .into_iter()
            .map(|(material, need)| {
                let have_qty = *have_lower.get(&material.to_lowercase()).unwrap_or(&0);
                RequirementLine {
                    material,
                    need,
                    have: have_qty,
                }
            })
            .collect();
        lines.sort_by(|a, b| a.material.cmp(&b.material));
        let fully_met = lines.iter().all(|l| l.have >= l.need);
        GapReport {
            module_type: module_type.to_string(),
            name: name.to_string(),
            grade: target_grade,
            lines,
            fully_met,
        }
    }

    /// Materials for an experimental effect (one application, no grade),
    /// diffed against inventory. `None` if the effect does not exist for
    /// the module. Reported with `grade: 0` so callers can tell it apart.
    pub fn experimental_gap(
        &self,
        module_type: &str,
        name: &str,
        have: &HashMap<String, i64>,
    ) -> Option<GapReport> {
        let bp = self.find_experimental(module_type, name)?;
        let have_lower: HashMap<String, i64> =
            have.iter().map(|(k, v)| (k.to_lowercase(), *v)).collect();
        let mut lines: Vec<RequirementLine> = bp
            .ingredients
            .iter()
            .map(|ing| RequirementLine {
                material: ing.name.clone(),
                need: ing.quantity,
                have: *have_lower.get(&ing.name.to_lowercase()).unwrap_or(&0),
            })
            .collect();
        lines.sort_by(|a, b| a.material.cmp(&b.material));
        let fully_met = lines.iter().all(|l| l.have >= l.need);
        Some(GapReport {
            module_type: module_type.to_string(),
            name: name.to_string(),
            grade: 0,
            lines,
            fully_met,
        })
    }

    pub fn gap_report(
        &self,
        module_type: &str,
        name: &str,
        from_grade: i64,
        target_grade: i64,
        have: &HashMap<String, i64>,
    ) -> GapReport {
        let totals = self.cumulative_requirements(module_type, name, from_grade, target_grade);
        let have_lower: HashMap<String, i64> =
            have.iter().map(|(k, v)| (k.to_lowercase(), *v)).collect();

        let mut lines: Vec<RequirementLine> = totals
            .into_iter()
            .map(|(material, need)| {
                let have_qty = *have_lower.get(&material.to_lowercase()).unwrap_or(&0);
                RequirementLine {
                    material,
                    need,
                    have: have_qty,
                }
            })
            .collect();
        lines.sort_by(|a, b| a.material.cmp(&b.material));
        let fully_met = lines.iter().all(|l| l.have >= l.need);

        GapReport {
            module_type: module_type.to_string(),
            name: name.to_string(),
            grade: target_grade,
            lines,
            fully_met,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_the_full_vendored_dataset() {
        let cat = Catalog::load();
        assert!(
            cat.len() > 1000,
            "expected the full EDEngineer blueprint set, got {}",
            cat.len()
        );
    }

    #[test]
    fn finds_a_known_fsd_blueprint_by_type_name_grade() {
        let cat = Catalog::load();
        let bp = cat
            .find("Frame Shift Drive", "Increased FSD Range", 5)
            .expect("grade 5 FSD range must exist");
        assert!(bp
            .ingredients
            .iter()
            .any(|i| i.name == "Datamined Wake Exceptions"));
    }

    #[test]
    fn realistic_requirements_roll_each_lower_grade_n_times() {
        let cat = Catalog::load();
        let min = cat.cumulative_requirements("Frame Shift Drive", "Increased FSD Range", 0, 3);
        let real = cat.cumulative_requirements_realistic(
            "Frame Shift Drive",
            "Increased FSD Range",
            0,
            3,
            false,
        );
        // Grade 1 x1, grade 2 x2, grade 3 x1 (target): strictly more than the minimum.
        let sum = |m: &HashMap<String, i64>| m.values().sum::<i64>();
        assert!(
            sum(&real) > sum(&min),
            "realistic {} vs minimum {}",
            sum(&real),
            sum(&min)
        );
        assert_eq!(Catalog::rolls_to_unlock_next(4), 4);
    }

    #[test]
    fn cumulative_requirements_sum_across_grades() {
        let cat = Catalog::load();
        let totals = cat.cumulative_requirements("Frame Shift Drive", "Increased FSD Range", 0, 2);
        // grade 1 needs 1x Atypical Disrupted Wake Echoes, grade 2 needs 1 more
        assert_eq!(totals.get("Atypical Disrupted Wake Echoes"), Some(&2));
    }

    #[test]
    fn gap_report_flags_missing_materials() {
        let cat = Catalog::load();
        let mut have = HashMap::new();
        have.insert("Atypical Disrupted Wake Echoes".to_string(), 1);
        let report = cat.gap_report("Frame Shift Drive", "Increased FSD Range", 0, 2, &have);
        assert!(!report.fully_met);
        let line = report
            .lines
            .iter()
            .find(|l| l.material == "Atypical Disrupted Wake Echoes")
            .unwrap();
        assert_eq!(line.need, 2);
        assert_eq!(line.have, 1);
    }
}

#[cfg(test)]
mod synthesis_tests {
    use super::*;

    /// The library parses, covers the page, and agrees with the FSD
    /// injection grades the planner has carried in code since day one.
    #[test]
    fn synthesis_library_loads_and_matches_the_injection_table() {
        let c = Catalog::load();
        assert!(
            c.synthesis_recipes().len() >= 25,
            "{} recipes",
            c.synthesis_recipes().len()
        );
        assert!(c.synthesis_provenance().source.contains("Synthesis"));
        let fsd = c.find_synthesis("FSD Injection").expect("FSD Injection");
        assert_eq!(fsd.grades.len(), 3);
        let mats = |g: &str| -> Vec<String> {
            fsd.grades
                .iter()
                .find(|x| x.grade == g)
                .unwrap()
                .ingredients
                .iter()
                .map(|i| i.name.to_ascii_lowercase())
                .collect()
        };
        assert_eq!(mats("Basic"), vec!["carbon", "vanadium", "germanium"]);
        assert_eq!(
            mats("Standard"),
            vec!["carbon", "vanadium", "germanium", "cadmium", "niobium"]
        );
        assert_eq!(
            mats("Premium"),
            vec![
                "carbon",
                "germanium",
                "arsenic",
                "niobium",
                "yttrium",
                "polonium"
            ]
        );
        assert_eq!(fsd.grades[2].bonus.as_deref(), Some("+100% Jump Range"));
        assert!(
            c.find_synthesis("injection").is_some(),
            "unique substring lands"
        );
        assert!(
            c.find_synthesis("munitions").is_none(),
            "ambiguous substring does not"
        );
        // Every ingredient has a positive count and a name the material
        // catalog could look up (no wiki markup leaked through).
        for r in c.synthesis_recipes() {
            for g in &r.grades {
                assert!(!g.ingredients.is_empty(), "{} {}", r.name, g.grade);
                for i in &g.ingredients {
                    assert!(
                        i.count > 0 && !i.name.contains('[') && !i.name.contains('|'),
                        "{} {} {:?}",
                        r.name,
                        g.grade,
                        i
                    );
                }
            }
        }
    }
}
