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
                               hiss-timing hiss-accepted! hiss-held! hiss-resume! hiss-warm!
                               hiss-config hiss-config-problem)))

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

;; Where the user is: the focused view and its document. Two places compare
;; with `equal?` (helix-view/src/extension.rs gives both ids an equality).
(define (adapter.place)
   (let ([view (editor-focus)])
      (list view (editor->doc-id view))))

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

;; What is showing: the text (or #f), the view and document it was drawn in,
;; the char index it was drawn at, the document's length then, which tells a
;; typed char from an auto-pair, and how many chars before the cursor it
;; replaces. A ghost whose line belongs
;; further left than the cursor (two stray spaces on an empty line, the
;; reply at column 0) is drawn from where it goes, so the spaces it replaces
;; sit after it.
(define ghost-text #f)
(define ghost-place #f)
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
(define (hint-remove! index length text)
   (let* ([moved (+ index (- (adapter.length) length))]
          [cursor (adapter.cursor)])
      (adapter.remove-hint! index text)
      (unless (or (= moved index) (< moved 0))
         (adapter.remove-hint! moved text))
      (unless (or (= cursor index) (= cursor moved))
         (adapter.remove-hint! cursor text))))

;; A ghost in a view that lost the focus (a mouse click into another split)
;; cannot be removed from here, since hints are removed in the focused view.
;; It is kept as `(place index length text)` and removed the next time its
;; view has the focus.
(define orphan #f)

(define (orphan-sweep!)
   (when (and orphan (equal? (list-ref orphan 0) (adapter.place)))
      (hint-remove! (list-ref orphan 1) (list-ref orphan 2) (list-ref orphan 3))
      (set! orphan #f)))

(define (ghost-hide!)
   (orphan-sweep!)
   (when ghost-text
      (if (equal? (adapter.place) ghost-place)
          (hint-remove! ghost-index ghost-length ghost-text)
          (set! orphan (list ghost-place ghost-index ghost-length ghost-text))))
   (set! ghost-text #f))

;; Whether the ghost still stands where it was drawn: the same view and
;; document, the cursor where it was and no edit since. A mouse click and a
;; completion taken from the menu move the cursor or change the text without
;; running a command, so no hook of hiss's clears the ghost for them.
(define (ghost-fits?)
   (and ghost-text
        (equal? (adapter.place) ghost-place)
        (= (adapter.cursor) (+ ghost-index ghost-back))
        (= (adapter.length) ghost-length)))

(define (ghost-show! index text back)
   (ghost-hide!)
   (set! ghost-place (adapter.place))
   (set! ghost-index index)
   (set! ghost-length (adapter.length))
   (set! ghost-text text)
   (set! ghost-back back)
   (adapter.add-hint! index text))

;;; requests

;; The text around the cursor at `index`, as libhiss wants it.
(define (context-at index)
   (adapter.text-around index context-before context-after))

;; The request whose pieces may still be drawn; a counter that makes the
;; callbacks of older requests do nothing; where the request was made (the
;; view and document, the char index, the document's length); and what has
;; been typed since, through a ghost or ahead of one.
(define request-id #f)
(define generation 0)
(define request-place #f)
(define request-length 0)
(define origin 0)
(define typed "")

;; A request is held when the user typed on before it showed anything, or
;; typed its ghost to the end while it still streams. Its reply is still
;; right for the new text if it starts with what was typed, so it is kept
;; rather than cancelled and asked for again: `impo` is usually followed by
;; `r`, as the reply expects. `held` says that such a request's verdict is
;; still out, for the log. `asks` makes older debounce timers do nothing.
;; `want-ask` says that the debounce ran out while a request was held: the
;; pause the user makes before a request has happened, so a reply that
;; turns out wrong is followed by a new request at once.
(define held #f)
(define asks 0)
(define want-ask #f)

;; Whether the text is still as the request left it, but for what was typed
;; through: its pieces are drawn only then (see `ghost-fits?`).
(define (request-fits?)
   (and (equal? (adapter.place) request-place)
        (= (adapter.cursor) (+ origin (string-length typed)))
        (= (adapter.length) (+ request-length (string-length typed)))))

;; Cancels the request and removes the ghost, leaving a running debounce
;; timer alone.
(define (drop!)
   (set! generation (+ generation 1))
   (when request-id
      (native.hiss-cancel! request-id)
      (set! request-id #f))
   (set! typed "")
   (set! held #f)
   (ghost-hide!))

(define (clear!)
   (set! asks (+ asks 1))
   (set! want-ask #f)
   (drop!))

;; The reply does not go with what was typed, or ended before it showed
;; anything: the request goes, and a new one is made at once when the user
;; has paused meanwhile. Otherwise the debounce timer, still running, asks.
(define (miss!)
   (let ([ask want-ask])
      (when held
         (native.hiss-held! #f (string-length typed)))
      (drop!)
      (when ask
         (request! #f))))

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
                   (when message (adapter.status! message)))
                ;; Typed ahead of a reply that never caught up.
                (unless (or ghost-text (equal? typed ""))
                   (miss!))]
               [(not (request-fits?)) (clear!)]
               [(and (> (car piece) 0) (equal? typed ""))
                (ghost-show! (- origin (car piece)) (cadr piece) (car piece))
                (follow! id gen report)]
               [(and (= (car piece) 0) (starts-with? (cadr piece) typed))
                (let ([rest (substring (cadr piece) (string-length typed))])
                   (if (equal? rest "")
                       (ghost-hide!)
                       (begin
                          (when held
                             (native.hiss-held! #t (string-length typed))
                             (set! held #f)
                             (set! want-ask #f))
                          (ghost-show! (+ origin (string-length typed)) rest 0))))
                (follow! id gen report)]
               ;; The reply is shorter than what was typed so far, or the
               ;; cleaning holds it back: nothing to show yet.
               [(and (= (car piece) 0) (starts-with? typed (cadr piece)))
                (ghost-hide!)
                (follow! id gen report)]
               [else (miss!)])))))

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
             (set! request-place (adapter.place))
             (set! request-length (adapter.length))
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
         (clear!)
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
;;
;; When the wait is over and a held request is still on its way, nothing is
;; sent: `want-ask` notes the pause, and the reply decides (`miss!`). A ghost
;; that showed meanwhile is that request's, and stays.
(define (schedule! wait)
   (when enabled
      (set! asks (+ asks 1))
      (set! want-ask #f)
      (let* ([ask asks]
             [still? (lambda ()
                        (and enabled (= ask asks) (adapter.insert-mode?) (adapter.one-cursor?)))])
         (adapter.after! 0
            (lambda ()
               (when (and (still?) (not (show-cached!)))
                  (adapter.after! wait
                     (lambda ()
                        (when (still?)
                           (cond
                              [ghost-text void]
                              [request-id (set! want-ask #t)]
                              [else (request! #f)]))))))))))

;;; typing

;; A typed char that matches the ghost's first char shortens the ghost and
;; keeps the request. A ghost typed to its end is asked for again at once,
;; unless its request still streams and may bring more. A char typed while a
;; request has shown nothing yet is kept beside it (see `held`). Anything
;; else clears and asks again. An auto-pair adds two chars, so it counts
;; neither as typing through nor as typing ahead.
(define (on-char char)
   (cond
      [(and ghost-text
            (char=? char (string-ref ghost-text 0))
            (= (adapter.cursor) (+ ghost-index 1))
            (= (adapter.length) (+ ghost-length 1)))
       (let ([rest (substring ghost-text 1)])
          (set! typed (string-append typed (string char)))
          (cond
             [(not (equal? rest "")) (ghost-show! (adapter.cursor) rest 0)]
             [request-id
              (ghost-hide!)
              (schedule! 0)]
             [else
              (clear!)
              (schedule! 0)]))]
      [(and request-id (not ghost-text))
       (set! typed (string-append typed (string char)))
       (if (request-fits?)
           (set! held #t)
           (clear!))
       (schedule! debounce-ms)]
      [else
       (clear!)
       (schedule! debounce-ms)]))

;; Commands that edit text in insert mode ask again; any other command (a
;; cursor move, a mode switch) only clears. hiss's own commands are left alone.
(define edit-commands
   '("insert_newline" "delete_char_backward" "delete_char_forward"
     "delete_word_backward" "delete_word_forward" "kill_to_line_start"
     "kill_to_line_end" "insert_tab" "smart_tab" "insert_register"))

;; A command outside insert mode often comes shortly before an edit, so it
;; lets libhiss open the connection again if the API has been left alone
;; for a while (`hiss-warm!` does nothing otherwise).
(define (on-command name)
   (unless (starts-with? name "hiss")
      (clear!)
      (when (member name edit-commands)
         (schedule! debounce-ms))
      (when (and enabled (not (adapter.insert-mode?)))
         (native.hiss-warm!))))

(define (on-mode-switch event)
   (clear!)
   (when (adapter.entered-insert? event)
      (when config-problem
         (adapter.status! config-problem)
         (set! config-problem #f))
      (schedule! 0)))

;; The cursor moved or the text changed. Typing, commands and hiss's own
;; drawing leave the ghost and the request fitting, so this clears only
;; after what no other hook sees. It runs after the key was handled.
(define (on-selection view)
   (orphan-sweep!)
   (when (or (and ghost-text (not (ghost-fits?)))
             (and request-id (not (request-fits?))))
      (clear!)))

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
   (if (and (ghost-fits?) (adapter.one-cursor?))
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
(adapter.on-hook! "selection-did-change" on-selection)
