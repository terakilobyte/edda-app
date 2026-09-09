// Friendly names for the installed Piper voices. Anything not listed falls
// back to a cleaned-up file name.
const NAMES = {
  "en_GB-alan-medium": "Alan · British",
  "en_GB-alba-medium": "Alba · Scottish",
  "en_GB-cori-high": "Cori · British",
  "en_GB-jenny_dioco-medium": "Jenny · Irish",
  "en_GB-northern_english_male-medium": "Ned · Northern English",
  "en_GB-aru-medium": "Aru · British",
  "en_GB-semaine-medium": "Semaine · British",
  "en_GB-southern_english_female-low": "Sophie · Southern English",
  "en_GB-vctk-medium": "VCTK · British",
  "en_US-amy-medium": "Amy · American",
  "en_US-lessac-high": "Elise · American",
  "en_US-lessac-medium": "Elise · American",
  "en_US-ryan-high": "Ryan · American",
  "en_US-ryan-medium": "Ryan · American",
  "en_US-joe-medium": "Joe · American",
  "en_US-kristin-medium": "Kristin · American",
  "en_US-kusal-medium": "Kusal · American",
  "en_US-libritts-high": "Libri · American",
  "en_US-hfc_female-medium": "Harper · American",
  "en_US-hfc_male-medium": "Hank · American",
  "en_US-danny-low": "Danny · American",
  "en_US-arctic-medium": "Arctic · American",
  "en_US-bryce-medium": "Bryce · American",
  "en_US-john-medium": "John · American",
  "en_US-norman-medium": "Norman · American",
};

// Kokoro voice ids: a/b = American/British, f/m = female/male, then a name.
export function serverVoiceName(id) {
  const m = /^([ab])([fm])_(\w+)$/.exec(id ?? "");
  if (!m) return id ?? "";
  const who = m[3].replace(/^\w/, (c) => c.toUpperCase());
  return `${who} · ${m[1] === "a" ? "American" : "British"} ${m[2] === "f" ? "female" : "male"}`;
}

export function voiceName(model) {
  if (!model) return "";
  const key = model.replace(/\.onnx$/, "");
  if (NAMES[key]) return NAMES[key];
  const m = key.match(/^([a-z]{2})_([A-Z]{2})-(.+?)(?:-(low|medium|high))?$/);
  if (!m) return key;
  const who = m[3].replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
  const region = { GB: "British", US: "American" }[m[2]] ?? m[2];
  return `${who} · ${region}`;
}
