# MILE-2 notes

Full wording moved out of `docs/roadmap/INDEX.md` on 2026-11-12, when its line ran past 300 characters.

## ROAD-7 owner-approval (dropped 2026-11-02)

- ~~**ROAD-7**~~ owner-approval — dropped 2026-11-02: a deletion from a target dir costs at most a rebuild and holds no sensitive data, an agent can already delete a target by hand, and the locks and the apply-time recheck are what keep a build sound, so a human step added work without adding protection. Agents apply plans of every class (ADR-0002).
