;; hiss: grey ghost text for Helix, from the Anthropic API.
;;
;; The Steel half of hiss: it asks libhiss for a completion and draws it at
;; the cursor. The design is in .claude/spec.md (05/08).
;;
;; While hiss is on, every edit in insert mode schedules a request after a
;; short pause (the debounce). The reply streams in as an inlay hint, line
;; breaks included, so a multi-line ghost pushes the following text down.
;; Typing what the ghost shows shortens it without asking again; `:hiss-accept`
;; (bound to tab) inserts it, and anything else clears it.

(require "helix/editor.scm")
(require "helix/misc.scm")
(require (prefix-in helix.static. "helix/static.scm"))
(require-builtin helix/core/text as text.)

(#%require-dylib "libhiss"
   (prefix-in native. (only-in hiss-version hiss-start! hiss-next hiss-cancel! hiss-cached hiss-error
                               hiss-timing hiss-accepted! hiss-resume! hiss-config
                               hiss-config-problem)))

(provide hiss-on
         hiss-off
         hiss-toggle
         hiss-accept
         hiss-try
         hiss-test
         hiss-clear)

;;; adapter
;;
;; Every call into the Steel fork of Helix lives here, so a fork update
;; breaks only this section. Names checked against the fork's source at
;; 424add10 (helix-term/src/commands/engine/steel/). Notes on the rest of the
;; fork API are in .claude/spec.md (05/08).

(define (adapter.rope)
   (editor->text (editor->doc-id (editor-focus))))

;; The primary cursor, as a char index.
(define (adapter.cursor) (cursor-position))

;; The document's length in chars.
(define (adapter.length) (text.rope-len-chars (adapter.rope)))

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

;; A mode compares equal to its name as a symbol (helix-view/src/extension.rs).
(define (adapter.insert-mode?) (equal? (editor-mode) 'insert))
(define (adapter.entered-insert? event) (equal? (mode-switch-new event) 'insert))

(define (adapter.add-hint! index text) (add-inlay-hint index text))
(define (adapter.remove-hint! index text) (remove-inlay-hint index text))

;; Inserts `text` as it is at every cursor, with no auto-indent.
(define (adapter.insert! text) (helix.static.insert_string text))

;; Deletes from the cursor back to the line start, when only whitespace
;; comes before it (helix-term/src/commands.rs, kill_to_line_start).
(define (adapter.kill-to-line-start!) (helix.static.kill_to_line_start))

;; The text from the start of the cursor's line up to `index`.
(define (adapter.line-before index)
   (let* ([rope (adapter.rope)]
          [start (text.rope-line->char rope (text.rope-char->line rope index))])
      (text.rope->string (text.rope->slice rope start index))))

;; What tab does without hiss.
(define (adapter.tab!) (helix.static.smart_tab))

;; Whether the document has a single cursor (helix/static.scm,
;; current-selection-object and selection->ranges). Inserting acts on every
;; cursor while the ghost belongs to the primary one, so hiss keeps quiet
;; with more.
(define (adapter.one-cursor?)
   (= (length (helix.static.selection->ranges (helix.static.current-selection-object))) 1))

(define (adapter.status! message) (set-status! message))

(define (adapter.on-hook! event callback) (register-hook! event callback))

(define (adapter.await future callback) (helix-await-callback future callback))

;; Runs `thunk` on the main thread after `ms` milliseconds.
(define (adapter.after! ms thunk) (enqueue-thread-local-callback-with-delay ms thunk))

;;; settings
;;
;; From ~/.config/hiss/config.toml, which libhiss reads when it loads (see
;; examples/config.toml). A line it cannot read keeps its default, and the
;; status line names it the first time insert mode starts (a message set
;; while the editor starts up is soon written over).

(define enabled (native.hiss-config "enabled"))
(define debounce-ms (native.hiss-config "debounce"))

;; How much text around the cursor to hand libhiss: one char more on each
;; side than it sends, so that it can tell a line it must cut from the
;; start or end of the file.
(define context-before (+ (native.hiss-config "context_before") 1))
(define context-after (+ (native.hiss-config "context_after") 1))
(define config-problem (native.hiss-config-problem))

;;; the ghost

;; What is showing: the text (or #f), the char index it was drawn at, the
;; document's length then, which tells a typed char from an auto-pair, and
;; how many chars before the cursor it replaces. A ghost whose line belongs
;; further left than the cursor (two stray spaces on an empty line, the
;; reply at column 0) is drawn from where it goes, so the spaces it replaces
;; sit after it.
(define ghost-text #f)
(define ghost-index 0)
(define ghost-length 0)
(define ghost-back 0)

;; Helix moves inlay hints along with edits, and a hint goes after text
;; inserted at its own index (Assoc::After, helix-view/src/document.rs). So
;; by the time a hook clears the ghost, one edit has left it either where it
;; was (an edit after it) or shifted by the change in length (a typed char,
;; Enter and its indent, an auto-pair, backspace). Removing it only where it
;; was drawn would leave it on screen for good, so it is removed at both, and
;; at the cursor for good measure.
(define (ghost-hide!)
   (when ghost-text
      (let* ([moved (+ ghost-index (- (adapter.length) ghost-length))]
             [cursor (adapter.cursor)])
         (adapter.remove-hint! ghost-index ghost-text)
         (unless (or (= moved ghost-index) (< moved 0))
            (adapter.remove-hint! moved ghost-text))
         (unless (or (= cursor ghost-index) (= cursor moved))
            (adapter.remove-hint! cursor ghost-text))))
   (set! ghost-text #f))

(define (ghost-show! index text back)
   (ghost-hide!)
   (set! ghost-index index)
   (set! ghost-length (adapter.length))
   (set! ghost-text text)
   (set! ghost-back back)
   (adapter.add-hint! index text))

;;; requests

;; The text around the cursor at `index`, as libhiss wants it.
(define (context-at index)
   (adapter.text-around index context-before context-after))

;; The request whose pieces may still be drawn; a counter that makes stale
;; callbacks (older requests, older debounce timers) do nothing; the char
;; index the request was made at; and what has been typed through since.
(define request-id #f)
(define generation 0)
(define origin 0)
(define typed "")

(define (clear!)
   (set! generation (+ generation 1))
   (when request-id
      (native.hiss-cancel! request-id)
      (set! request-id #f))
   (set! typed "")
   (ghost-hide!))

;; Draws each piece of request `id`, minus what was typed through meanwhile.
;; A piece is `(back text)`: the text so far, and how many chars before the
;; cursor it starts. With `report`, the status line shows the timing at the
;; end; errors always.
(define (follow! id gen report)
   (adapter.await (native.hiss-next id)
      (lambda (piece)
         (when (= gen generation)
            (cond
               [(not piece)
                (set! request-id #f)
                (let ([message (or (native.hiss-error)
                                   (and report (or (native.hiss-timing) "hiss: nothing")))])
                   (when message (adapter.status! message)))]
               [(and (> (car piece) 0) (equal? typed ""))
                (ghost-show! (- origin (car piece)) (cadr piece) (car piece))
                (follow! id gen report)]
               [(and (= (car piece) 0) (starts-with? (cadr piece) typed))
                (let ([rest (substring (cadr piece) (string-length typed))])
                   (if (equal? rest "")
                       (ghost-hide!)
                       (ghost-show! (+ origin (string-length typed)) rest 0)))
                (follow! id gen report)]
               [else (clear!)])))))

;; Asks for a completion at the cursor, replacing any older one. A request
;; made with `report` is the user's own; the others are automatic, and
;; libhiss turns those down without a word while a refusal that would come
;; back every time (no key, a model the API does not know) stands, having
;; said it once.
(define (request! report)
   (clear!)
   (let* ([index (adapter.cursor)]
          [around (context-at index)]
          [id (native.hiss-start! (car around) (cadr around) (adapter.language) (adapter.path)
                                  (not report))])
      (if id
          (begin
             (set! request-id id)
             (set! origin index)
             (follow! id generation report))
          (let ([message (or (native.hiss-error) (and report "hiss: no request"))])
             (when message (adapter.status! message))))))

;; Draws the suggestion libhiss already made for the text around the
;; cursor, if there is one, and says whether it did.
(define (show-cached!)
   (let* ([index (adapter.cursor)]
          [around (context-at index)]
          [hit (native.hiss-cached (car around) (cadr around) (adapter.language) (adapter.path))])
      (when hit
         (set! origin index)
         (ghost-show! (- index (car hit)) (cadr hit) (car hit)))
      (if hit #t #f)))

;; Shows a suggestion made before for this very text (after an undo, or a
;; deleted char typed again), or else asks after `wait` milliseconds, unless
;; something happened in between. The first step waits for the current
;; command to finish its edit (`o` inserts its line after the mode switch).
;;
;; Typing waits for the debounce, since more keys are likely to follow. Tab,
;; a ghost typed to its end and a fresh insert mode come after a pause the
;; user already made, so they ask at once.
(define (schedule! wait)
   (when enabled
      (let* ([gen generation]
             [still? (lambda ()
                        (and enabled (= gen generation) (adapter.insert-mode?) (adapter.one-cursor?)))])
         (adapter.after! 0
            (lambda ()
               (when (and (still?) (not (show-cached!)))
                  (adapter.after! wait
                     (lambda ()
                        (when (still?)
                           (request! #f))))))))))

;;; typing

;; A typed char that matches the ghost's first char shortens the ghost and
;; keeps the request; anything else clears and asks again. An auto-pair adds
;; two chars, so it does not count as typing through.
(define (on-char char)
   (if (and ghost-text
            (char=? char (string-ref ghost-text 0))
            (= (adapter.cursor) (+ ghost-index 1))
            (= (adapter.length) (+ ghost-length 1)))
       (let ([rest (substring ghost-text 1)])
          (set! typed (string-append typed (string char)))
          (if (equal? rest "")
              (begin (clear!) (schedule! 0))
              (ghost-show! (adapter.cursor) rest 0)))
       (begin (clear!) (schedule! debounce-ms))))

;; Commands that edit text in insert mode ask again; any other command (a
;; cursor move, a mode switch) only clears. hiss's own commands are left alone.
(define edit-commands
   '("insert_newline" "delete_char_backward" "delete_char_forward"
     "delete_word_backward" "delete_word_forward" "kill_to_line_start"
     "kill_to_line_end" "insert_tab" "smart_tab"))

(define (on-command name)
   (unless (starts-with? name "hiss")
      (clear!)
      (when (member name edit-commands)
         (schedule! debounce-ms))))

(define (on-mode-switch event)
   (clear!)
   (when (adapter.entered-insert? event)
      (when config-problem
         (adapter.status! config-problem)
         (set! config-problem #f))
      (schedule! 0)))

;;; commands

;;@doc
;; Turn completion on: hiss asks as you type in insert mode, also after an
;; error that made it stop asking.
(define (hiss-on)
   (set! enabled #t)
   (native.hiss-resume!)
   (adapter.status! "hiss: on")
   void)

;;@doc
;; Turn completion off and remove the ghost.
(define (hiss-off)
   (set! enabled #f)
   (clear!)
   (adapter.status! "hiss: off")
   void)

;;@doc
;; Turn completion on or off.
(define (hiss-toggle)
   (if enabled (hiss-off) (hiss-on)))

;;@doc
;; Insert the ghost at the cursor. With no ghost showing, do what tab does.
(define (hiss-accept)
   (if (and ghost-text (adapter.one-cursor?))
       (let* ([text ghost-text]
              [back ghost-back]
              [lead (adapter.line-before (adapter.cursor))]
              [kept (substring lead 0 (max 0 (- (string-length lead) back)))])
          (clear!)
          (when (> back 0)
             (adapter.kill-to-line-start!)
             (unless (equal? kept "")
                (adapter.insert! kept)))
          (adapter.insert! text)
          (native.hiss-accepted! (string-length text))
          (schedule! 0))
       (begin
          (clear!)
          (adapter.tab!)
          (schedule! debounce-ms)))
   void)

;;@doc
;; Ask for a completion at the cursor now, and show its timing when it ends.
(define (hiss-try)
   (if (adapter.one-cursor?)
       (request! #t)
       (adapter.status! "hiss: one cursor only"))
   void)

;;@doc
;; Draw a fixed three-line sample at the cursor.
(define (hiss-test)
   (clear!)
   (ghost-show! (adapter.cursor) "first line of the ghost\n   second line\n   third line" 0)
   void)

;;@doc
;; Remove the ghost and cancel the request.
(define (hiss-clear)
   (clear!)
   void)

;;; hooks

(adapter.on-hook! "post-insert-char" on-char)
(adapter.on-hook! "post-command" on-command)
(adapter.on-hook! "on-mode-switch" on-mode-switch)
