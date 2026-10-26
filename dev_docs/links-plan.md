# Links: pulling things into a message

## Why

Today `#name` in a message is found by scanning the text: any `#word`
matching a tag's name is expanded, picked or not, pasted C code
included (`#include` with a tag named "include"). Nothing marks what
will be expanded, and only tags can be pulled in. The user wants one
general way to link anything into a message (a tag, a file, lines of a
file, a symbol), with control over whether its contents go in.

## Decided (with the user, 2026-10-23)

- **A message is text plus links.** A link marks a span of the text and
  points at a tag or a thing (`Subject`: a file, lines, a symbol, a
  terminal command, a chat turn, a plan part...). Nothing scans the text:
  a `#word` that isn't a link is plain text.
- **Each side its own job.** The typing box knows only "this span is a
  link" (drawn as a chip in the link's colour; editing inside it drops
  the link). Pickers belong to their owners (`#`: tags; `@`: files and
  symbols from the code index). The server asks each thing's owner for
  its content (the things layer, as tags do now) and checks the reader
  may read it. Tags resolve a tag link to its things.
- **Contents in, or a pointer.** Each link is Auto, Whole or Pointer.
  Auto: whole when small (up to 4 KiB and 100 lines), else a pointer.
  A pointer is the thing's name, its size, its first lines, and how to
  read the rest (`thing_read`, a new tool, reading any thing by name,
  live). Whole is a snapshot at send. The chip shows which: plain for
  Auto, filled for Whole, underlined for Pointer. A key in the box
  (`ctrl+i`) cycles the link at the caret. (Built this way: shift+Enter
  in the picker was dropped; the key after picking does it.)
- **A budget per message.** What's forced Whole past the budget goes as
  a pointer. Said under the box before it's sent: the shell asks the
  server a moment after the box's last change (`/api/links/check`, the
  same expansion a send does) when a link is linked whole. (First built
  as "said after sending"; the user called that out; fixed, b3178b3.)
- **A draft keeps its links** (`drafts.links`): it comes back linked.
- **The picker never eats typing.** `#` always opens the tag picker
  (no tags: it says so). Leaving it without picking puts what was typed
  in its box back after the `#`; a space with nothing matching does too.

## Defaults for the open questions (say if otherwise)

1. **Agents' own messages** (`slot_send`, discussion replies, reports,
   queue briefs) are plain text: no links, nothing scanned. (So what an
   agent passes on is never read for it: the reach check that expanded
   an agent's `#tag` "only as far as the slot may read" has nothing left
   to check, and is gone.) An agent that wants
   another to see a thing names it; the reader uses `thing_read` or
   `tag_read`.
2. **What `@` offers:** files and symbols (the code index's search);
   `path:10-20` typed in the picker links those lines.

## Not in this

- Chips in the chat's history: a sent message shows its text as sent
  (with what was pulled in after it, as now). Showing chips there needs
  the turns' own record of links; a later step.
- Other kinds behind `@` (terminal commands, chat turns, plan items):
  they're linked by tagging them; a picker for them is later.

## Steps

1. Contracts: `Link { start, end, to: Linked, include: Include }`,
   `Linked::{Tag, Thing}`, `Include::{Auto, Whole, Pointer}`.
2. Server: things render what's linked (`Things::attach`), tags give a
   tag's things; the slot manager's expand takes links; the prompt route,
   discussions (stored with what's said) and `tell` carry them; the
   `#word` scan goes. `thing_read` tool. Tests.
3. Cockpit: the typing box's links (spans kept through edits, drawn as
   chips); `#` and `@` pickers put links; include cycling; the chat, the
   discussion box and the palette's writing send links; the picker
   fixes. Tests.
4. README, snapshots, suite, lints; one rebuild; live in the user's
   browser together with the tags review batch; self-review.
