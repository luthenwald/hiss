;; How a user loads hiss, in ~/.config/helix/init.scm.
;; `just hx` runs the fork with this directory as its Steel config.

(require "helix/keymaps.scm")
(require "hiss/hiss.scm")

;; For comparing the two ways of drawing the ghost: C-space asks for a
;; suggestion at the cursor; :hiss-draw hint or :hiss-draw overlay switches.
(add-global-keybinding
   (hash "insert"
      (hash "C-space" ":hiss-try")))
