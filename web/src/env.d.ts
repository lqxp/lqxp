/// <reference types="vite/client" />

interface QxpRtcRuntimeConfig {
  relayOnly?: boolean;
  turnUrls?: string[];
  turnUsername?: string;
  turnCredential?: string;
  callsEnabled?: boolean;
  callsUnavailableReason?: string;
}

interface QxpRuntimeConfig {
  rtc?: QxpRtcRuntimeConfig;
}

declare global {
  interface Window {
    __QXP_RUNTIME__?: QxpRuntimeConfig;
  }
}

declare module "*.vue" {
  import type { DefineComponent } from "vue";

  const component: DefineComponent<Record<string, unknown>, Record<string, unknown>, unknown>;
  export default component;
}

export {};
