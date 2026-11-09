;; hiss: grey ghost text for Helix, from the Anthropic API.
;;
;; The Steel half of hiss: it asks libhiss for a completion and draws it at
;; the cursor. The design is in .claude/spec.md (05/08).
;;
;; For now it draws the ghost in two ways, to compare them (roadmap 03/06):
;; as an inlay hint, which pushes the following text down but puts the cursor
;; after the ghost, or as an overlay, which keeps the cursor before the ghost
;; but covers the lines below. `:hiss-draw hint` and `:hiss-draw overlay`
;; switch; `:hiss-try` asks for a real completion, `:hiss-test` shows a sample.

(require "helix/editor.scm")
(require "helix/misc.scm")
(require "helix/components.scm")
(require (prefix-in helix.static. "helix/static.scm"))
(require-builtin helix/core/text as text.)

(#%require-dylib "libhiss"
   (prefix-in native. (only-in hiss-version hiss-start! hiss-next hiss-cancel! hiss-error hiss-timing)))

(provide hiss-try
         hiss-test
         hiss-clear
         hiss-draw)

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

;; The cursor's column in its line, counted in graphemes.
(define (adapter.column) (helix.static.get-current-column-number))

;; Up to `before` chars before `index` and `after` chars after it, as two
;; strings. libhiss trims both at line boundaries.
(define (adapter.text-around index before after)
   (let* ([rope (adapter.rope)]
          [end (text.rope-len-chars rope)])
      (list (text.rope->string (text.rope->slice rope (max 0 (- index before)) index))
            (text.rope->string (text.rope->slice rope index (min end (+ index after)))))))

;; The text from `index` to the end of its line, without the line break.
(define (adapter.line-rest index)
   (let* ([rope (adapter.rope)]
          [line (text.rope-char->line rope index)]
          [stop (if (< (+ line 1) (text.rope-len-lines rope))
                    (- (text.rope-line->char rope (+ line 1)) 1)
                    (text.rope-len-chars rope))])
      (text.rope->string (text.rope->slice rope index (max index stop)))))

(define (adapter.path)
   (or (editor-document->path (editor->doc-id (editor-focus))) ""))

(define (adapter.language)
   (or (editor-document->language (editor->doc-id (editor-focus))) ""))

;; The cursor's screen position, or #f when it is off screen.
(define (adapter.screen-cursor) (car (current-cursor)))

;; The focused view's area on screen, statusline included.
(define (adapter.view-area) (editor-focused-buffer-area))

(define (adapter.add-hint! index text) (add-inlay-hint index text))
(define (adapter.remove-hint! index text) (remove-inlay-hint index text))

(define (adapter.status! message) (set-status! message))

(define (adapter.on-hook! event callback) (register-hook! event callback))

(define (adapter.await future callback) (helix-await-callback future callback))

(define (adapter.push-overlay! name render)
   (push-component! (new-component! name #f render (hash))))

(define (adapter.style scope) (theme-scope-ref scope))

(define (adapter.draw! frame x y string style) (frame-set-string! frame x y string style))

;;; the ghost

;; How the ghost is drawn: 'hint or 'overlay.
(define ghost-style 'hint)

;; What is showing: the text (or #f), the char index it belongs at, the rest
;; of that line after the index, and the cursor's column when it was drawn.
(define ghost-text #f)
(define ghost-index 0)
(define ghost-rest "")
(define ghost-column 0)

(define overlay-name "hiss-ghost")
(define overlay-pushed #f)

(define (ghost-hide!)
   (when (and ghost-text (eq? ghost-style 'hint))
      (adapter.remove-hint! ghost-index ghost-text))
   (set! ghost-text #f))

(define (ghost-show! index text)
   (ghost-hide!)
   (set! ghost-index index)
   (set! ghost-rest (adapter.line-rest index))
   (set! ghost-column (adapter.column))
   (set! ghost-text text)
   (if (eq? ghost-style 'hint)
       (adapter.add-hint! index text)
       (unless overlay-pushed
          (adapter.push-overlay! overlay-name overlay-render)
          (set! overlay-pushed #t))))

;;; the overlay

;; Draws `string` at (x, y) in `style`, cut at the right edge `right`.
(define (draw-clipped! frame x y string style right)
   (let ([room (- right x)])
      (when (> room 0)
         (adapter.draw! frame x y
            (if (> (string-length string) room) (substring string 0 room) string)
            style))))

;; Draws the ghost over the buffer: the first line from the cursor, the others
;; from the text area's left edge, then the rest of the cursor's line after the
;; last ghost line, as it will read once accepted. Each row it touches is
;; blanked to the right edge first, so old text does not show through. Every
;; coordinate stays inside the view: a drawing error makes the fork drop the
;; component for good. The left edge comes from the cursor's column, which is
;; wrong on a soft-wrapped line; the overlay then starts too far left.
(define (overlay-render state area frame)
   (let* ([cursor (and ghost-text (eq? ghost-style 'overlay) (adapter.screen-cursor))]
          [view (and cursor (adapter.view-area))])
      (when view
         (let* ([right (+ (area-x view) (area-width view))]
                [bottom (- (+ (area-y view) (area-height view)) 1)]
                [edge (area-x view)]
                [left (max edge (- (position-col cursor) ghost-column))]
                [ghost (adapter.style "ui.virtual.inlay-hint")]
                [plain (adapter.style "ui.text")]
                [lines (split-many ghost-text "\n")]
                [final (- (length lines) 1)])
            (let loop ([i 0] [lines lines])
               (unless (null? lines)
                  (let ([x (if (= i 0) (max edge (position-col cursor)) left)]
                        [y (+ (position-row cursor) i)]
                        [line (car lines)])
                     (when (< y bottom)
                        (draw-clipped! frame x y (make-string (max 0 (- right x)) #\space) plain right)
                        (draw-clipped! frame x y line ghost right)
                        (when (= i final)
                           (draw-clipped! frame (+ x (string-length line)) y ghost-rest plain right))))
                  (loop (+ i 1) (cdr lines))))))))

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

;;@doc
;; Choose how the ghost is drawn: `hint` (an inlay hint) or `overlay`.
(define (hiss-draw . args)
   (if (null? args)
       (adapter.status! (string-append "hiss: drawing as " (symbol->string ghost-style)))
       (let ([style (string->symbol (car args))])
          (if (or (eq? style 'hint) (eq? style 'overlay))
              (begin
                 (clear!)
                 (set! ghost-style style)
                 (adapter.status! (string-append "hiss: drawing as " (car args))))
              (adapter.status! "hiss: draw as hint or overlay")))))

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
