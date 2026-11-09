;; hiss: grey ghost text for Helix, from the Anthropic API.
;;
;; The Steel half of hiss: it asks libhiss for a completion and draws it at
;; the cursor. The design is in .claude/spec.md (05/08).
;;
;; The ghost is an inlay hint, line breaks included, so a multi-line ghost
;; pushes the following text down. For now `:hiss-try` asks for a completion
;; at the cursor and `:hiss-test` shows a sample; the loop that asks as you
;; type comes next (roadmap 04/06).

(require "helix/editor.scm")
(require "helix/misc.scm")
(require-builtin helix/core/text as text.)

(#%require-dylib "libhiss"
   (prefix-in native. (only-in hiss-version hiss-start! hiss-next hiss-cancel! hiss-error hiss-timing)))

(provide hiss-try
         hiss-test
         hiss-clear)

;;; adapter
;;
;; Every call into the Steel fork of Helix lives here, so a fork update
;; breaks only this section. Names checked against the fork's source at
;; 424add10 (helix-term/src/commands/engine/steel/). Notes on the rest of the
;; fork API hiss will need are in .claude/spec.md (05/08).

(define (adapter.rope)
   (editor->text (editor->doc-id (editor-focus))))

;; The primary cursor, as a char index.
(define (adapter.cursor) (cursor-position))

;; Up to `before` chars before `index` and `after` chars after it, as two
;; strings. libhiss trims both at line boundaries.
(define (adapter.text-around index before after)
   (let* ([rope (adapter.rope)]
          [end (text.rope-len-chars rope)])
      (list (text.rope->string (text.rope->slice rope (max 0 (- index before)) index))
            (text.rope->string (text.rope->slice rope index (min end (+ index after)))))))

(define (adapter.path)
   (or (editor-document->path (editor->doc-id (editor-focus))) ""))

(define (adapter.language)
   (or (editor-document->language (editor->doc-id (editor-focus))) ""))

(define (adapter.add-hint! index text) (add-inlay-hint index text))
(define (adapter.remove-hint! index text) (remove-inlay-hint index text))

(define (adapter.status! message) (set-status! message))

(define (adapter.on-hook! event callback) (register-hook! event callback))

(define (adapter.await future callback) (helix-await-callback future callback))

;;; the ghost

;; What is showing: the text (or #f) and the char index it was drawn at.
(define ghost-text #f)
(define ghost-index 0)

;; Helix moves inlay hints along with edits, so by the time a hook clears the
;; ghost, an edit at the cursor (Enter, a typed char, backspace) has carried
;; the hint to the new cursor position. Removing it only where it was drawn
;; would leave it on screen for good, so it is removed there and at the cursor.
(define (ghost-hide!)
   (when ghost-text
      (adapter.remove-hint! ghost-index ghost-text)
      (let ([cursor (adapter.cursor)])
         (unless (= cursor ghost-index)
            (adapter.remove-hint! cursor ghost-text))))
   (set! ghost-text #f))

(define (ghost-show! index text)
   (ghost-hide!)
   (set! ghost-index index)
   (set! ghost-text text)
   (adapter.add-hint! index text))

;;; requests

;; The request whose pieces may still be drawn, and a counter that makes
;; callbacks from older requests drop their pieces.
(define request-id #f)
(define generation 0)

(define (clear!)
   (set! generation (+ generation 1))
   (when request-id
      (native.hiss-cancel! request-id)
      (set! request-id #f))
   (ghost-hide!))

(define (follow! id gen index)
   (adapter.await (native.hiss-next id)
      (lambda (text)
         (when (= gen generation)
            (if text
                (begin
                   (ghost-show! index text)
                   (follow! id gen index))
                (begin
                   (set! request-id #f)
                   (adapter.status! (or (native.hiss-error) (native.hiss-timing) "hiss: nothing"))))))))

;;; commands

;;@doc
;; Ask for a completion at the cursor and draw it as it streams in.
(define (hiss-try)
   (clear!)
   (let* ([index (adapter.cursor)]
          [around (adapter.text-around index 10000 2500)]
          [id (native.hiss-start! (car around) (cadr around) (adapter.language) (adapter.path))])
      (if id
          (begin
             (set! request-id id)
             (follow! id generation index))
          (adapter.status! (or (native.hiss-error) "hiss: no request"))))
   void)

;;@doc
;; Draw a fixed three-line sample at the cursor.
(define (hiss-test)
   (clear!)
   (ghost-show! (adapter.cursor) "first line of the ghost\n   second line\n   third line")
   void)

;;@doc
;; Remove the ghost and cancel the request.
(define (hiss-clear)
   (clear!)
   void)

;;; hooks

;; For now the ghost goes on any edit, cursor move or mode switch, so it never
;; outlives the text it was made for. hiss's own commands are left alone, or
;; a key bound to :hiss-try would clear its own ghost.
(adapter.on-hook! "post-insert-char" (lambda (char) (clear!)))
(adapter.on-hook! "on-mode-switch" (lambda (event) (clear!)))
(adapter.on-hook! "post-command"
   (lambda (name)
      (unless (starts-with? name "hiss")
         (clear!))))
