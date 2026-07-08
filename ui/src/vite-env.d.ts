/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_API_BASE?: string;
  readonly VITE_FIXTURES?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
