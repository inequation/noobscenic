# Instructions for agents

Read [`README.md`](README.md) — the human-facing description of this project.

## Tracking progress

[`doc/PLAN.md`](doc/PLAN.md) is the implementation plan, and its **section 15
("Milestones")** is a live checklist rather than a static document. As you finish an
item, tick its box in that section (`- [ ]` → `- [x]`).

Whenever you tick a box, **commit** — the work and the ticked checklist in the same
commit — so the repository history and the checklist never disagree about what is done.

Ticking a box also means rewriting the one-sentence **status** line at the end of
[`README.md`](README.md) to match where the project now stands, so that someone who
reads only the README is not misled. Keep it to a single sentence, and put it in that
same commit.

## Glossary

Whenever you invent a new term for an abstraction in this project (a name for a
component, pattern, or concept that isn't already documented), add a plain-language
explanation of it to [`GLOSSARY.md`](GLOSSARY.md) in the same commit that introduces
the term.
