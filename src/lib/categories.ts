// The one list of cue categories -- what the Add cue sheet's picker shows,
// and where creation.svelte.ts's SKIP_CATEGORIES / LANGUAGE_KINDS get their
// keys from, so a category only has to be added here. `key` is the string
// stored in the filter file, so existing keys are never renamed: older
// files (and the sample filters) keep working untouched.
//
// `action` is the default the picker applies when a category is chosen --
// language is dialogue, so it mutes; everything else skips. The sheet still
// lets each cue flip it.

export type CueAction = "mute" | "skip";

export interface CategoryItem {
  key: string;
  name: string;
  action: CueAction;
}

export interface CategoryGroup {
  id: string;
  label: string;
  // SVG path data (24x24 viewBox, stroked) for the group's icon.
  icon: string;
  items: CategoryItem[];
}

const mute = (key: string, name: string): CategoryItem => ({ key, name, action: "mute" });
const skip = (key: string, name: string): CategoryItem => ({ key, name, action: "skip" });

// Credits & extras comes first on purpose: it's the group most often used
// when back-filling a title, and nothing else in the list is quicker to
// reach for.
export const CATEGORY_GROUPS: CategoryGroup[] = [
  {
    id: "credits",
    label: "Credits & extras",
    icon: "M5 5h14M8 10h8M7 14h10M9 18h6",
    items: [skip("opening-credits", "Opening credits"), skip("credits", "End credits"), skip("recap", "Recap")],
  },
  {
    id: "language",
    label: "Language",
    icon: "M4 5h16v10H9l-5 4z",
    items: [
      mute("language-profanity", "Profanity"),
      mute("language-blasphemy", "Blasphemy"),
      mute("language-crude", "Crude language"),
      mute("language-sexual", "Sexual language"),
      mute("language-slurs", "Slurs & insults"),
      mute("language-childish", "Childish language"),
    ],
  },
  {
    id: "sex",
    label: "Sex & Nudity",
    icon: "M12 20s-7-4.4-7-10a4 4 0 0 1 7-2.6A4 4 0 0 1 19 10c0 5.6-7 10-7 10z",
    items: [skip("nudity", "Nudity"), skip("sex", "Sex scenes"), skip("intimacy", "Intimacy"), skip("revealing-attire", "Revealing attire")],
  },
  {
    id: "violence",
    label: "Violence",
    icon: "M12 3v5M12 16v5M3 12h5M16 12h5M6 6l3 3M15 15l3 3M18 6l-3 3M9 15l-3 3",
    items: [
      skip("violence", "Fighting & violence"),
      skip("weapons", "Guns & weapons"),
      skip("gore", "Blood & gore"),
      skip("death", "Death"),
      skip("abuse", "Cruelty & abuse"),
    ],
  },
  {
    id: "scary",
    label: "Scary",
    icon: "M12 4l9 15H3zM12 10v4M12 17v.5",
    items: [skip("frightening", "Frightening"), skip("jump-scare", "Jump scares"), skip("peril", "Peril"), skip("occult", "Occult")],
  },
  {
    id: "substances",
    label: "Substances",
    icon: "M7 4h10l-1 8a4 4 0 0 1-8 0zM12 16v4M9 20h6",
    items: [skip("drugs & alcohol", "Drugs & alcohol"), skip("smoking", "Smoking")],
  },
  {
    id: "themes",
    label: "Humor & themes",
    icon: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM8.5 14a4 4 0 0 0 7 0M9 9.5v.5M15 9.5v.5",
    items: [skip("crude humor", "Crude humor"), skip("self-harm", "Self-harm & suicide")],
  },
];

// The picker's last tab: a category typed by hand. Not in CATEGORY_GROUPS
// since it has no fixed items.
export const CUSTOM_GROUP = {
  id: "custom",
  label: "Custom",
  icon: "M4 20l4-1 11-11-3-3L5 16zM14 7l3 3",
};

export function findCategory(key: string): { group: CategoryGroup; item: CategoryItem } | null {
  for (const group of CATEGORY_GROUPS) {
    const item = group.items.find((i) => i.key === key);
    if (item) return { group, item };
  }
  return null;
}
