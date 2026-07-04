# Phase 8 — Performance UI

> Read [`00-roadmap.md`](./00-roadmap.md) first. **This phase is
> deliberately under-specified** — it should be driven by what daily
> rehearsal actually surfaces, not built speculatively. Treat the sections
> below as candidates to validate against real use, not a committed backlog.

## Goal

Once the loop is trustworthy (Tier 1 exit test) and gig-hardened (phase 6),
the next lever on "does this replace Qt at a gig" is the on-stage
experience itself: visibility from a distance, one-handed control while the
other hand codes, and surviving a dark room.

## Why this phase is ordered last among the "build now" items

Unlike gig-hardening or teaching mode, there's no strong a priori feature
list here — Qt doesn't have a distinguished "performance mode" either, so
there's no parity target to hit, and guessing at performer needs without
performer data risks building the wrong three things well instead of the
right one thing adequately. **Do not start this phase until at least a few
weeks of logged daily/rehearsal use exist to point at.**

## Candidate features (validate before building)

### Full-screen / distraction-free mode
A single keybinding that hides the header chrome down to Run/Stop/BPM,
maximizes the editor, and optionally auto-collapses the dock panes
(mechanism already exists — `collapsed: HashSet<&'static str>` in
`SonicSpike`, or the newer `hidden_panels` from the View settings toggles).
Cheap, low-risk, do first if anything.

### Projector-scale type
The font-zoom mechanism (Ctrl+=/-/0, `zoom()` in `main.rs`) already exists
and persists; this candidate is really "is 40pt (the current clamp max)
enough for a back-of-the-room projector, and does the UI chrome (buttons,
line numbers) scale with it or stay tiny and unreadable next to giant
code." Test the existing clamp before assuming new work is needed here —
this might already be done.

### MIDI-controller-mappable actions
SuperSonic's native MIDI subsystem already exposes everything needed over
OSC — no new MIDI crate required:
- `/midi/notify/subscribe` → pushes `/midi/in/<verb> s:port [i:channel]
  <args…>` for incoming note/CC/clock messages (see
  `app/external/supersonic/docs/OSC_API.md`, "MIDI" section).
- Add a `ClientEvent::MidiIn { port, verb, channel, args }` parse arm in
  `rust-core/src/protocol.rs` (mirrors the existing `MIDI_IN_PORTS` handling)
  and a `Session::subscribe_midi_notify()` sender.
- A settings-pane "MIDI mapping" table: pick an action (Run, Stop, volume
  up/down, buffer next/prev — the same set already reachable via
  `Cmd`/keybindings in `main.rs`) and "learn" a controller message by
  listening for the next `/midi/in/*` event and recording its
  port+channel+CC/note number. Persist in prefs.conf like everything else
  (`"midi_map_run" = "port:channel:cc60"` or similar).
- Dispatch: the tick loop's event-drain already has a natural hook point
  next to the existing `ClientEvent::Status`/`ClientEvent::Cue` handling.

### Scope on a second monitor
GPUI supports multiple windows (`cx.open_window` is already called once in
`main()`); a second borderless window showing just the scope/spectrum,
positioned on a second display, is a plausible ask for a live-visual setup.
Validate demand before building — this is the most speculative candidate
here.

## Work items (only after validation)

1. Pick the 1-2 highest-signal candidates from actual rehearsal notes.
2. Prototype behind a settings toggle (never on by default — a performance
   mode that surprises the user mid-set is worse than not having it).
3. Runtime-verify each addition the established way (unit test what's pure,
   `make e2e`/manual smoke for anything touching the real runtime).

## Exit criteria

There isn't a fixed one — this phase's "done" is "the features that got
built are the ones actually used in a real rehearsal or gig," which is a
judgment call for Tom, not a checklist.

## Risks

- **Building ahead of evidence.** The single biggest risk in this phase is
  starting it too early. If there isn't yet a concrete "I wanted X and
  couldn't" moment from real use, that's the signal to wait, not to
  brainstorm harder.
