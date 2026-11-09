;; hiss: grey ghost text for Helix, from the Anthropic API.
;;
;; The Steel half of hiss: it watches keystrokes, asks libhiss for a
;; completion, and draws it at the cursor. The design is in
;; .claude/spec.md (05/08).

;;; adapter
;;
;; Every call into the Steel fork of Helix lives here, so a fork update
;; breaks only this section. Names checked against the fork's source at
;; 424add10 (helix-term/src/commands/engine/steel/).
;;
;; Hooks, from helix/core/editor:
;;    (register-hook event-name callback), with string event names:
;;    "post-insert-char"      callback gets the inserted char
;;    "post-command"          callback gets the command name (a string)
;;    "selection-did-change"  callback gets the view id
;;    "on-mode-switch"        callback gets an event; read it with
;;                            mode-switch-old and mode-switch-new
;;    "document-focus-lost", "document-changed", "document-closed", ...
;;
;; Editor state:
;;    (cursor-position)                 primary cursor, char index (helix/misc.scm)
;;    (editor-focus) (editor->doc-id view) (editor->text doc-id) -> rope
;;    (editor-document->path doc-id)    path or #f
;;    (editor-mode)                     current mode
;;    rope->slice, rope-char->line, rope-line->char, rope->string (helix/core/text)
;;
;; Ghost text, from helix/misc.scm (focused view only):
;;    (add-inlay-hint char-index text) -> (first-line . last-line)
;;    (remove-inlay-hint char-index text) removes every plugin hint at that index
;;    remove-inlay-hint-by-id clears the whole hint set of the view, LSP hints
;;    included, so hiss does not use it.
;;
;; Text: (insert_string text) inserts at every cursor (helix/core/static).
;; Tab's usual action is the static command smart_tab.
;;
;; Threads and time:
;;    (helix-await-callback future callback) awaits a libhiss future and runs
;;    callback on the main thread with the editor context.
;;    (enqueue-thread-local-callback-with-delay ms thunk) runs thunk on the
;;    main thread after ms milliseconds; the delay must be an integer.
;;    hx.with-context and hx.block-on-task (helix/ext.scm) for native threads.
;;
;; Commands: any top-level definition is a typed command, :hiss-... .
;; Keys: (add-global-keybinding (hash "insert" (hash "tab" ":hiss-accept"))).

(require "helix/misc.scm")

(#%require-dylib "libhiss" (prefix-in native. (only-in hiss-version hiss-ping)))

(provide hiss-ping)

;;; proof of the native module (roadmap 01/06)

;;@doc
;; Show the version of libhiss, after a round trip through its async path
;; that ends on the main thread.
(define (hiss-ping)
   (helix-await-callback
      (native.hiss-ping 50)
      (lambda (reply)
         (set-status! (string-append "hiss: " reply " from libhiss " (native.hiss-version))))))
