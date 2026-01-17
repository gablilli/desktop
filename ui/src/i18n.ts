import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import Backend from "i18next-http-backend";
import LanguageDetector from "i18next-browser-languagedetector";

i18n
  .use(Backend)
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    fallbackLng: "en-US",
    debug: import.meta.env.DEV,
    ns: ["common"],
    load: "currentOnly",
    defaultNS: "common",
    interpolation: {
      escapeValue: false,
    },
    backend: {
      loadPath: "/locales/{{lng}}/{{ns}}.json",
    },
  });

i18n.on("languageChanged", (lng) => {
  document.documentElement.setAttribute("lang", lng);
});

export const languages = [
  {
    code: "en-US",
    displayName: "English",
  },
  {
    code: "zh-CN",
    displayName: "简体中文",
  },
  {
    code: "zh-TW",
    displayName: "繁體中文",
  },
  {
    code: "ja-JP",
    displayName: "日本語",
  },
];

export default i18n;
