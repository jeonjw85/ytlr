import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { english } from "./locales/en";

export type Language = "en" | "ko";
export const languageKey = "ytlr.ui-language";
type Values = Record<string, string | number>;

// Korean source strings are kept as catalog keys; service diagnostics without a
// catalog entry remain verbatim so no error information is lost.
export function translate(
  language: Language,
  source: string,
  values: Values = {},
): string {
  const text =
    language === "en" && Object.hasOwn(english, source)
      ? english[source]
      : source;
  return text.replace(/\{(\w+)\}/g, (match, key: string) =>
    Object.hasOwn(values, key) ? String(values[key]) : match,
  );
}

function savedLanguage(): Language {
  try {
    return localStorage.getItem(languageKey) === "ko" ? "ko" : "en";
  } catch {
    return "en";
  }
}

type I18n = {
  language: Language;
  dateLocale: string;
  setLanguage: (language: Language) => void;
  t: (source: string, values?: Values) => string;
};
const LanguageContext = createContext<I18n | null>(null);

export function LanguageProvider({ children }: { children: ReactNode }) {
  const [language, updateLanguage] = useState<Language>(savedLanguage);
  const setLanguage = useCallback((next: Language) => {
    localStorage.setItem(languageKey, next);
    updateLanguage(next);
  }, []);
  const t = useCallback(
    (source: string, values?: Values) => translate(language, source, values),
    [language],
  );

  useEffect(() => {
    document.documentElement.lang = language;
    if (isTauri()) {
      void invoke("set_ui_language", { language }).catch(console.error);
    }
  }, [language]);

  const value = useMemo(
    () => ({
      language,
      dateLocale: language === "ko" ? "ko-KR" : "en-US",
      setLanguage,
      t,
    }),
    [language, setLanguage, t],
  );
  return (
    <LanguageContext.Provider value={value}>
      {children}
    </LanguageContext.Provider>
  );
}

export function useI18n() {
  const context = useContext(LanguageContext);
  if (!context) throw new Error("LanguageProvider is required");
  return context;
}
