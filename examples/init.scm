;; How a user loads hiss, in ~/.config/helix/init.scm.
;; `just hx` runs the fork with this directory as its Steel config.

(require "helix/keymaps.scm")
(require "hiss/hiss.scm")

;; For now hiss asks only when told to: C-space asks for a suggestion at the
;; cursor. Asking as you type comes with the next step.
(add-global-keybinding
   (hash "insert"
      (hash "C-space" ":hiss-try")))
