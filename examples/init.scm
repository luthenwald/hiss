;; How a user loads hiss, in ~/.config/helix/init.scm.
;; `just hx` runs the fork with this directory as its Steel config.

(require "helix/keymaps.scm")
(require "hiss/hiss.scm")

;; hiss starts on; :hiss-toggle, :hiss-on and :hiss-off switch it.
;; (hiss-configure #:debounce 150 #:log #t)

;; Tab takes the ghost, and indents as usual when there is none.
;; C-space asks right away and shows how long the answer took.
(add-global-keybinding
   (hash "insert"
      (hash "tab" ":hiss-accept"
            "C-space" ":hiss-try")))
