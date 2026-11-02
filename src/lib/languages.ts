// Languages for audio and subtitles, by the ISO 639-2 code the backend turns
// every tag into ("fre" whether a file says "fra", "fr" or "fre").

export const LANGUAGES: [code: string, name: string][] = [
  ["eng", "English"],
  ["fre", "French"],
  ["spa", "Spanish"],
  ["ger", "German"],
  ["ita", "Italian"],
  ["por", "Portuguese"],
  ["dut", "Dutch"],
  ["swe", "Swedish"],
  ["nor", "Norwegian"],
  ["dan", "Danish"],
  ["fin", "Finnish"],
  ["pol", "Polish"],
  ["rus", "Russian"],
  ["ukr", "Ukrainian"],
  ["tur", "Turkish"],
  ["gre", "Greek"],
  ["ara", "Arabic"],
  ["heb", "Hebrew"],
  ["hin", "Hindi"],
  ["jpn", "Japanese"],
  ["kor", "Korean"],
  ["chi", "Chinese"],
];

// ISO 639-2 has two codes for some languages; the system's names know the other one.
const TERMINOLOGY: Record<string, string> = { cze: "ces", rum: "ron", ice: "isl", may: "msa", alb: "sqi", arm: "hye", baq: "eus", bur: "mya", geo: "kat", mac: "mkd", mao: "mri", per: "fas", slo: "slk", tib: "bod", wel: "cym" };

let names: Intl.DisplayNames | null | undefined;

/** "French" for "fre", the system's English name for others, else the code in capitals. */
export function languageName(code: string | null | undefined) {
  if (!code) return null;
  const listed = LANGUAGES.find(([c]) => c === code)?.[1];
  if (listed) return listed;
  if (names === undefined) {
    try {
      names = new Intl.DisplayNames(["en"], { type: "language" });
    } catch {
      names = null;
    }
  }
  try {
    const name = names?.of(TERMINOLOGY[code] ?? code);
    if (name && name.toLowerCase() !== code) return name;
  } catch {
    // Not a code the system knows.
  }
  return code.toUpperCase();
}
