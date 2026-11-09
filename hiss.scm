;; hiss: grey ghost text for Helix, from the Anthropic API.
;;
;; The Steel half of hiss: it watches keystrokes, asks libhiss for a
;; completion, and draws it at the cursor. The design is in
;; .claude/spec.md (05/08); nothing is wired up yet.

;;; adapter
;;
;; Every call into the Steel fork of Helix lives here, so a fork update
;; breaks only this section. Names are confirmed in roadmap step 01.
