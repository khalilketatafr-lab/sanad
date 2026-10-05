/**
 * Capture shield (src/reader/capture-shield.ts): the opt-in client deterrent.
 * The ladder and veil *rules* are pure and tested directly; a tiny fake DOM
 * confirms the installer raises graduated signals on PrintScreen/copy and
 * blanks the page the moment the window loses focus.
 */
import assert from "node:assert/strict";
import { test } from "node:test";

import {
  DEFAULT_CAPTURE_SHIELD,
  NO_STRIKES,
  canHideVeil,
  installCaptureShield,
  nextStrike,
  shouldVeil,
  strikeTier,
  type CaptureSignal,
} from "../src/reader/capture-shield.ts";

test("the strike ladder climbs to a latched block at the ceiling", () => {
  let s = NO_STRIKES;
  s = nextStrike(s, 3);
  assert.deepEqual(s, { strikes: 1, blocked: false });
  s = nextStrike(s, 3);
  assert.deepEqual(s, { strikes: 2, blocked: false });
  s = nextStrike(s, 3);
  assert.deepEqual(s, { strikes: 3, blocked: true });
  // Blocked latches.
  assert.equal(nextStrike(s, 3).blocked, true);
  assert.deepEqual([strikeTier(1, 3), strikeTier(2, 3), strikeTier(3, 3)], ["warn", "final", "blocked"]);
});

test("the veil rule: veiled iff focus lost or pointer outside", () => {
  assert.equal(shouldVeil({ focusLost: false, pointerOutside: false }), false);
  assert.equal(shouldVeil({ focusLost: true, pointerOutside: false }), true);
  assert.equal(shouldVeil({ focusLost: false, pointerOutside: true }), true);
});

test("the veil cannot be hidden before the minimum shown time, nor while still wanted", () => {
  assert.equal(canHideVeil({ shownAt: 0, now: 50, minVeilMs: 100, wantVeil: false }), false, "too soon");
  assert.equal(canHideVeil({ shownAt: 0, now: 150, minVeilMs: 100, wantVeil: false }), true, "settled");
  assert.equal(canHideVeil({ shownAt: 0, now: 150, minVeilMs: 100, wantVeil: true }), false, "still wanted");
});

// ── Fake DOM for the installer ──────────────────────────────────────────────

class FakeEl extends EventTarget {
  style: { cssText: string; display: string } = { cssText: "", display: "none" };
  children: FakeEl[] = [];
  setAttribute(): void {}
  appendChild(c: FakeEl): void {
    this.children.push(c);
  }
  remove(): void {}
  matches(): boolean {
    return false;
  }
}

class FakeDoc extends EventTarget {
  body = new FakeEl();
  documentElement = new FakeEl();
  visibilityState = "visible";
  createElement(): FakeEl {
    return new FakeEl();
  }
}

function fakeWindow(): EventTarget & { document: FakeDoc } {
  const target = new EventTarget() as EventTarget & Record<string, unknown>;
  const doc = new FakeDoc();
  target["document"] = doc;
  target["navigator"] = { userAgent: "", maxTouchPoints: 0 };
  target["setTimeout"] = () => 0;
  target["clearTimeout"] = () => {};
  target["setInterval"] = () => 0;
  target["clearInterval"] = () => {};
  return target as unknown as EventTarget & { document: FakeDoc };
}

function keyup(key: string): Event {
  const e = new Event("keyup");
  Object.assign(e, { key, code: key, keyCode: key === "PrintScreen" ? 44 : 0 });
  return e;
}

test("installer raises graduated PrintScreen signals and a copy signal", () => {
  const win = fakeWindow();
  const signals: CaptureSignal[] = [];
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const shield = installCaptureShield(win as any, { veil: false, maxStrikes: 3 }, (s) => signals.push(s));

  win.document.dispatchEvent(keyup("PrintScreen"));
  win.document.dispatchEvent(keyup("PrintScreen"));
  win.document.dispatchEvent(keyup("PrintScreen"));
  assert.deepEqual(signals.map((s) => s.kind), ["printscreen", "printscreen", "printscreen"]);
  assert.deepEqual(signals.map((s) => s.strikes), [1, 2, 3]);
  assert.deepEqual(signals.map((s) => s.blocked), [false, false, true]);

  // A non-PrintScreen key does nothing.
  win.document.dispatchEvent(keyup("a"));
  assert.equal(signals.length, 3);

  const copy = new Event("copy");
  Object.assign(copy, { clipboardData: { setData: () => {} }, preventDefault: () => {} });
  win.document.dispatchEvent(copy);
  assert.equal(signals.at(-1)?.kind, "copy");

  shield.dispose();
  win.document.dispatchEvent(keyup("PrintScreen"));
  assert.equal(signals.length, 4, "no signals after dispose");
});

test("installer blanks the page the instant the window loses focus (veil on)", () => {
  const win = fakeWindow();
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const shield = installCaptureShield(win as any, { veil: true, veilColor: "#f7f4ec" }, () => {});

  win.dispatchEvent(new Event("blur"));
  const veil = win.document.body.children[0];
  if (veil === undefined) throw new Error("a veil element was inserted");
  assert.equal(veil.style.display, "block", "the veil covers the page on blur");
  assert.match(veil.style.cssText, /z-index:2147483647/u, "top of the stack");
  assert.match(veil.style.cssText, /#f7f4ec/u, "paper-coloured");

  shield.dispose();
});

test("defaults are off and conservative", () => {
  assert.equal(DEFAULT_CAPTURE_SHIELD.veil, false);
  assert.equal(DEFAULT_CAPTURE_SHIELD.veilOnMobile, false);
  assert.equal(DEFAULT_CAPTURE_SHIELD.maxStrikes, 3);
});
