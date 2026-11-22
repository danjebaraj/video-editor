<!-- planner:begin v6 — written by the Planner app; edits inside this block are replaced on upgrade -->
# Planner folder: rules for agents

This folder holds **feature specs** only (`type: feature`, ids `FEAT-…`). Bug reports and tasks belong in their own or a mixed planner folder.

Follow these rules exactly. When a rule and an item's own text disagree, the rule wins; ask the user if you are unsure.

## 1. Layout

- Every item is its own folder named `<ID>-<slug>`, for example `BUG-brave-otter-7K2M-login-crash/`.
- The item's text is `README.md` in that folder. Never rename `README.md`.
- Images go in the item's `images/` folder; every other file (Word, Excel, PowerPoint, PDF, Markdown, zip, text, logs) goes in its `attachments/` folder.
- Link images from `README.md` with relative paths: `![Login screen](images/login.png)`. Link files the same way: `[crash log](attachments/crash.log)`.
- `AGENTS.md`, `CLAUDE.md`, `README.md` and `.planner.toml` at the top of this folder are not items. Do not edit `.planner.toml`.
- Older items may be single `.md` files directly in this folder. Treat them as items too; do not move them unless asked.

## 2. Front matter

Every item's `README.md` starts with YAML front matter. Keys, in this order:

```yaml
---
id: BUG-brave-otter-7K2M   # required, unique in the whole repo, never changed once assigned
type: bug               # required: feature | bug | task
title: Login crashes on an empty password   # required; the first "# " heading repeats it
status: open            # required, see section 4
fix: none               # bugs: none | in-progress | fixed | wont-fix
verified: no            # bugs: no | yes
approved: no            # no | yes — set by people only, see section 4
priority: medium        # low | medium | high | critical
created: 2026-11-16     # YYYY-MM-DD, set once
updated: 2026-11-16     # YYYY-MM-DD, set to today on every change you make
attachments: []         # list of files, see section 5
---
```

- Features have `implementation: none | partial | done` instead of `fix` and `verified`.
- Tasks have `done: no | yes` instead of `fix` and `verified`.
- Keep keys you don't know about exactly as they are.
- Quote a value with double quotes when it contains `: `, starts with a special character, or looks like a number or `yes`/`no`.

## 3. IDs

- Format: the prefix `FEAT-`, `BUG-` or `TASK-`, two random lowercase words (an adjective, then an animal or other noun) and a four-character code, joined by `-`: `FEAT-sweet-salmon-46TF`, `BUG-brave-otter-7K2M`, `TASK-calm-heron-9XQ2`. Random names keep ids made on different branches from colliding.
- The code is four characters from `23456789ABCDEFGHJKMNPQRSTUVWXYZ` and has at least one digit.
- Older items have numbered ids (`FEAT-0007`, `BUG-0123`). They stay valid; never change them.
- To create an item, make up a new id this way and check that no item in **any** planner folder in the repo uses it (front matter `id:` values and item folder names). Do not number new items.
- Never reuse or change an existing id. If you find two items with the same id, tell the user; the Planner app gives the newer one a new id.
- Refer to other items by id (`see BUG-brave-otter-7K2M`), not by path.

## 4. Status and progress

Feature `status` values, in order: `proposed`, `planned`, `in-progress`, `partial`, `done`, `dropped`.
- `proposed`: written down, not agreed. `planned`: agreed, not started. `in-progress`: being worked on.
- `partial`: some requirements are implemented and merged, others are not. `done`: every requirement and acceptance criterion is checked. `dropped`: will not be done (say why in the log).

Bug `status` values, in order: `open`, `confirmed`, `in-progress`, `fixed`, `verified`, `wont-fix`, `duplicate`.
- `open`: reported. `confirmed`: reproduced. `in-progress`: being fixed. `fixed`: the fix is implemented. `verified`: the steps to reproduce no longer fail. `wont-fix` / `duplicate`: closed without a fix (for `duplicate`, name the other id in the log).

Task `status` values, in order: `todo`, `in-progress`, `done`, `dropped`.
- `todo`: written down, not started. `in-progress`: being worked on. `done`: every step and "done when" box is checked. `dropped`: will not be done (say why in the log).

**Approval.** Only items with `approved: yes` may be implemented. `approved` is set by people (in the Planner app, which also writes `approved-on: <date>`); never set or change it yourself.
- For an item that isn't approved, you may write or refine its text when asked (requirements, acceptance criteria, design notes), but do not write code for it, do not check its boxes, and do not move its status past `planned` (features), `confirmed` (bugs) or `todo` (tasks). Say that it needs approval instead.
- If approval is withdrawn (`approved: no`) while work is under way, stop and tell the user.

Implementation and fixes are recorded on their own, so "done" is never hidden behind another status:
- Features: `implementation: none` (nothing built), `partial` (some requirements built), `done` (all built).
- Bugs: `fix: none`, `in-progress`, `fixed` or `wont-fix`; and `verified: yes` only once the steps to reproduce no longer fail. A verified bug still says `fix: fixed`.
- Tasks: `done: no` until every box is checked, then `done: yes`.
- Status and these fields must agree: feature `partial` ↔ `implementation: partial`, `done` ↔ `implementation: done`; bug `fixed` ↔ `fix: fixed`, `verified: no`; bug `verified` ↔ `fix: fixed`, `verified: yes`; bug `wont-fix` ↔ `fix: wont-fix`; task `done` ↔ `done: yes`, any other task status ↔ `done: no`.

Checklists are the source of truth for progress:
- Each requirement, acceptance criterion, fix step and task step is one `- [ ]` line.
- When you implement something, check its box (`- [x]`) **in the same change** as the code, and only when it is fully implemented and tested. Never check a box for partial work; split the line into smaller boxes instead.
- Never delete an unchecked box to make an item look finished. If a requirement is dropped, keep it, leave it unchecked and add ` — dropped: <reason>` to the line.
- After updating boxes, set `status`:
  - Feature: no boxes checked → leave the status as it was (or `in-progress` if you started work); some checked → `partial` and `implementation: partial`; all checked → `done` and `implementation: done`.
  - Bug: fix implemented → `fixed` and `fix: fixed`; verified with the steps to reproduce → `verified` and `verified: yes` (keep `fix: fixed`).
  - Task: some checked → `in-progress`; all checked → `done` and `done: yes`.
- Set `updated` to today's date.
- Append one entry to `## Implementation log` (create the section at the end if it is missing):

  ```markdown
  ### 2026-11-16
  - Implemented: <what, by checkbox text>
  - Not done: <what is left, or "nothing">
  - Files: <main files changed>
  - Commit/PR: <hash or link, if known>
  ```

## 5. Attachments

- Add a file by copying it into `images/` or `attachments/` and adding an entry to `attachments:` in the front matter:

  ```yaml
  attachments:
    - file: attachments/crash.log   # path relative to the item folder
      kind: log                     # image | word | excel | powerpoint | pdf | markdown | archive | text | log | other
      size: 18233                   # bytes
      added: 2026-11-16
      description: Crash log from the first launch
  ```

- Never delete or overwrite an attachment unless the user asks. Read attachments for context when they help.

## 6. Writing items

- Keep the template's headings. Fill in the sections; don't leave placeholder comments in finished text.
- Features need concrete, testable requirements and acceptance criteria, one per checkbox.
- Bugs need numbered steps to reproduce, the expected result and the actual result.
- Tasks need concrete steps and "done when" checks, one per checkbox.
- Write so another agent could implement the item without asking questions. Spell out file names, inputs, outputs and edge cases.

## 7. Specs

- A `spec.md` at the top of the repo, or in a component folder such as `backend/spec.md`, describes the product or that component as a whole: goals, users, requirements and architecture. It has no front matter and no id, and it is not an item.
- Read the nearest `spec.md` before writing or implementing an item. Items are the units of work; the spec is the big picture they fit into.
- When an item changes something the spec describes, say so in the item's Design notes. Edit `spec.md` itself only when the user asks.
<!-- planner:end -->

<!-- Add your team's own notes for agents below this line. The Planner never changes them. -->
