// Overlay options live in the overlay's URL, so OBS keeps them with the browser source.

export interface OverlayOptions {
  /** Show donor messages. */
  messages: boolean;
  /** Longest message shown, in characters. */
  length: number;
  /** Seconds each alert stays on screen. */
  seconds: number;
  /** Include donations other programs make, such as pump.fun creator fees. */
  automatic: boolean;
  theme: "dark" | "light";
  /** Random key that lets the setup page send test alerts to this overlay. */
  key: string | null;
}

export const DEFAULTS: OverlayOptions = {
  messages: true,
  length: 120,
  seconds: 8,
  automatic: true,
  theme: "dark",
  key: null,
};

export const LIMITS = {
  length: { min: 20, max: 280 },
  seconds: { min: 3, max: 30 },
};

function clamp(value: number, { min, max }: { min: number; max: number }, fallback: number): number {
  return Number.isFinite(value) ? Math.min(max, Math.max(min, Math.round(value))) : fallback;
}

const KEY = /^[A-Za-z0-9_-]{16,64}$/;

export function readOptions(params: URLSearchParams): OverlayOptions {
  const flag = (name: string, fallback: boolean) => {
    const v = params.get(name);
    return v === null ? fallback : v !== "0" && v !== "off" && v !== "false";
  };
  const number = (name: string) => {
    const v = params.get(name);
    return v === null ? NaN : Number(v);
  };
  const key = params.get("key");
  return {
    messages: flag("messages", DEFAULTS.messages),
    length: clamp(number("length"), LIMITS.length, DEFAULTS.length),
    seconds: clamp(number("seconds"), LIMITS.seconds, DEFAULTS.seconds),
    automatic: flag("automatic", DEFAULTS.automatic),
    theme: params.get("theme") === "light" ? "light" : "dark",
    key: key && KEY.test(key) ? key : null,
  };
}

/** The overlay URL for a config. Only options that differ from the defaults are written. */
export function overlayUrl(origin: string, config: string, o: OverlayOptions): string {
  const params = new URLSearchParams();
  if (o.key) params.set("key", o.key);
  if (!o.messages) params.set("messages", "0");
  const length = clamp(o.length, LIMITS.length, DEFAULTS.length);
  if (length !== DEFAULTS.length) params.set("length", String(length));
  const seconds = clamp(o.seconds, LIMITS.seconds, DEFAULTS.seconds);
  if (seconds !== DEFAULTS.seconds) params.set("seconds", String(seconds));
  if (!o.automatic) params.set("automatic", "0");
  if (o.theme !== DEFAULTS.theme) params.set("theme", o.theme);
  const query = params.toString();
  return `${origin}/overlay/${encodeURIComponent(config)}${query ? `?${query}` : ""}`;
}

/** A random test key, 128 bits in URL-safe base64. */
export function newKey(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

// Links are removed so an overlay never shows a clickable-looking scam on stream. Anything
// with a scheme or "www." goes, and so does a bare domain on a common top-level domain.
const SCHEME_LINK = /\b(?:[a-z][a-z0-9+.-]*:\/\/|www\.)\S+/gi;
const TLDS =
  "com|net|org|io|xyz|gg|fun|app|co|me|ly|tv|so|sol|link|site|info|live|pro|ai|ru|cn|us|uk|to|cc|club|online|shop|store|finance|money|exchange|claims?|lol|top|vip|win|bet|casino|dev|tech|biz|in|de|fr|es|br|tk|ml|ga|cf|gq";
const BARE_LINK = new RegExp(`\\b(?:[a-z0-9-]+\\.)+(?:${TLDS})\\b(?:\\/\\S*)?`, "gi");
// Control characters, zero-width characters and bidirectional overrides, which can hide or
// reorder text.
const HIDDEN = /[\u0000-\u001f\u007f-\u009f­؜᠎​-‏‪-‮⁠-⁩﻿]/g;

/** A donor message made safe to show: no links or hidden characters, at most `max` characters. */
export function cleanMessage(text: string, max: number): string {
  const clean = text.replace(HIDDEN, " ").replace(SCHEME_LINK, "").replace(BARE_LINK, "").replace(/\s+/g, " ").trim();
  const chars = Array.from(clean);
  return chars.length > max ? `${chars.slice(0, max).join("").trimEnd()}…` : clean;
}
