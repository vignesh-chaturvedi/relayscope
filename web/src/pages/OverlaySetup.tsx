import { useEffect, useRef, useState } from "react";

const PREVIEW = { width: 640, height: 260 };
import { Link, useParams } from "react-router-dom";
import { post } from "../api";
import { LabelNames, Section } from "../components";
import { shortAddress } from "../format";
import { DEFAULTS, LIMITS, type OverlayOptions, newKey, overlayUrl } from "../overlay/options";

const CONFIG_ID = /^(?:0x)?[0-9a-f]{64}$|^[1-9A-HJ-NP-Za-km-z]{32,44}$/i;
const storageKey = (config: string) => `relayscope.overlay.${config}`;

// Options and the test key are kept per config in this browser, so the URL stays the same
// between visits. Storage can be missing (private windows), so every access is guarded.
function loadOptions(config: string): OverlayOptions {
  try {
    const saved = JSON.parse(localStorage.getItem(storageKey(config)) ?? "null") as Partial<OverlayOptions> | null;
    if (saved?.key) return { ...DEFAULTS, ...saved };
  } catch {
    // Fall through to fresh options.
  }
  return { ...DEFAULTS, key: newKey() };
}

function saveOptions(config: string, options: OverlayOptions) {
  try {
    localStorage.setItem(storageKey(config), JSON.stringify(options));
  } catch {
    // Not saved; the page still works.
  }
}

type TestState = { kind: "idle" } | { kind: "sending" } | { kind: "done"; text: string } | { kind: "error"; text: string };

function testResult(overlays: number): string {
  if (overlays === 0) return "Sent, but no overlay is open with this URL yet. Add it to OBS, then try again.";
  if (overlays === 1) return "Sent to 1 open overlay, counting the preview below.";
  return `Sent to ${overlays} open overlays, counting the preview below.`;
}

export default function OverlaySetupPage() {
  const { config = "" } = useParams();
  // Keyed by config, so moving between configs starts from that config's saved options.
  return <OverlaySetup key={config} config={config} />;
}

function OverlaySetup({ config }: { config: string }) {
  const [options, setOptions] = useState<OverlayOptions>(() => loadOptions(config));
  const [copied, setCopied] = useState(false);
  const [test, setTest] = useState<TestState>({ kind: "idle" });
  const urlField = useRef<HTMLInputElement>(null);
  const previewBox = useRef<HTMLDivElement>(null);
  const [scale, setScale] = useState(1);

  useEffect(() => saveOptions(config, options), [config, options]);

  // The preview is drawn at the size OBS uses and scaled down to fit narrow screens.
  useEffect(() => {
    const box = previewBox.current;
    if (!box) return;
    const observer = new ResizeObserver(([entry]) => setScale(Math.min(1, entry.contentRect.width / PREVIEW.width)));
    observer.observe(box);
    return () => observer.disconnect();
  }, []);

  if (!CONFIG_ID.test(config)) {
    return (
      <div className="notice">
        <h1>That is not a config ID</h1>
        <p>
          A config ID is 32 bytes in hex or base58. <Link to="/">Search for a config</Link> and open its page.
        </p>
      </div>
    );
  }

  const url = overlayUrl(window.location.origin, config, options);
  const update = (change: Partial<OverlayOptions>) => {
    setOptions((o) => ({ ...o, ...change }));
    setCopied(false);
  };

  async function copy() {
    try {
      await navigator.clipboard.writeText(url);
      setCopied(true);
    } catch {
      // No clipboard access: select the text so it can be copied by hand.
      urlField.current?.select();
    }
  }

  async function sendTest() {
    setTest({ kind: "sending" });
    try {
      const { overlays } = await post<{ overlays: number }>("/overlay/test", { config, key: options.key });
      setTest({ kind: "done", text: testResult(overlays) });
    } catch (error) {
      setTest({ kind: "error", text: error instanceof Error ? error.message : "The test alert was not sent." });
    }
  }

  function changeKey() {
    update({ key: newKey() });
    setTest({ kind: "idle" });
  }

  return (
    <>
      <div className="page-head">
        <p className="eyebrow">Stream overlay</p>
        <h1>Donation alerts for OBS</h1>
        <p className="meta">
          Config{" "}
          <Link to={`/config/${config}`} className="mono">
            {shortAddress(config, 6)}
          </Link>
        </p>
        <LabelNames configId={config} label={null} />
        <p className="lead">
          Every donation to this config shows on stream as it lands, usually within two seconds. The overlay has a transparent background, so it
          sits on top of your scene.
        </p>
      </div>

      <div className="two-col">
        <Section title="Options">
          <div className="form">
            <div className="field">
              <label className="check">
                <input type="checkbox" checked={options.messages} onChange={(e) => update({ messages: e.target.checked })} />
                Show donor messages
              </label>
              <p className="hint">Links and hidden characters are removed, and long messages are cut short. Donations made by other programs never show a message.</p>
            </div>
            <div className="field">
              <label htmlFor="length">Longest message, in characters</label>
              <input
                id="length"
                type="number"
                inputMode="numeric"
                min={LIMITS.length.min}
                max={LIMITS.length.max}
                value={Number.isFinite(options.length) ? options.length : ""}
                disabled={!options.messages}
                onChange={(e) => update({ length: e.target.valueAsNumber })}
              />
            </div>
            <div className="field">
              <label htmlFor="seconds">Seconds each alert stays up</label>
              <input
                id="seconds"
                type="number"
                inputMode="numeric"
                min={LIMITS.seconds.min}
                max={LIMITS.seconds.max}
                value={Number.isFinite(options.seconds) ? options.seconds : ""}
                onChange={(e) => update({ seconds: e.target.valueAsNumber })}
              />
            </div>
            <div className="field">
              <label className="check">
                <input type="checkbox" checked={options.automatic} onChange={(e) => update({ automatic: e.target.checked })} />
                Include donations made by other programs
              </label>
              <p className="hint">Such as creator fees that pump.fun sends on its own. Turn this off to show only people donating.</p>
            </div>
            <fieldset className="field">
              <legend>Alert style</legend>
              <div className="choices">
                {(["dark", "light"] as const).map((theme) => (
                  <label key={theme} className="check">
                    <input type="radio" name="theme" value={theme} checked={options.theme === theme} onChange={() => update({ theme })} />
                    {theme === "dark" ? "Dark card" : "Light card"}
                  </label>
                ))}
              </div>
            </fieldset>
          </div>
        </Section>

        <Section title="Add it to OBS">
          <ol className="steps">
            <li>
              In OBS, add a <strong>Browser</strong> source to your scene.
            </li>
            <li>Paste the URL below, and set the width to 640 and the height to 260.</li>
            <li>Send a test alert, then move and resize the source until it sits where you want.</li>
          </ol>
          <div className="field">
            <label htmlFor="overlay-url">Overlay URL</label>
            <div className="copy-row">
              <input id="overlay-url" ref={urlField} className="mono" readOnly value={url} onFocus={(e) => e.target.select()} />
              <button type="button" className="button" onClick={copy}>
                {copied ? "Copied" : "Copy"}
              </button>
            </div>
            <p className="hint">
              Keep this URL to yourself: anyone who has it can send test alerts to your overlay.{" "}
              <button type="button" className="link-button" onClick={changeKey}>
                Make a new URL
              </button>
            </p>
          </div>
          <div className="actions">
            <button type="button" className="button button-primary" onClick={sendTest} disabled={test.kind === "sending"}>
              {test.kind === "sending" ? "Sending" : "Send a test alert"}
            </button>
          </div>
          <p role="status" className={test.kind === "error" ? "status-error" : "muted"}>
            {test.kind === "done" || test.kind === "error" ? test.text : ""}
          </p>
        </Section>
      </div>

      <Section title="Preview">
        <p className="muted small">The checkered area is transparent in OBS. Test alerts show here too.</p>
        <div className="overlay-preview" ref={previewBox} style={{ height: PREVIEW.height * scale }}>
          <iframe
            key={url}
            title="Overlay preview"
            src={url}
            width={PREVIEW.width}
            height={PREVIEW.height}
            // Below full size it fills the box from the left; at full size it is centered.
            style={scale < 1 ? { transform: `scale(${scale})`, transformOrigin: "top left", margin: 0 } : undefined}
          />
        </div>
      </Section>
    </>
  );
}
