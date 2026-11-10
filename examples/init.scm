;; How to load hiss, in ~/.config/helix/init.scm.

(require "helix/keymaps.scm")
(require "hiss/hiss.scm")

;; hiss starts on; :hiss-toggle, :hiss-on and :hiss-off switch it.
;; Settings live in ~/.config/hiss/config.toml (see config.toml here).

;; Tab takes the ghost, and indents as usual when there is none.
;; C-space asks right away and shows how long the answer took.
(add-global-keybinding
   (hash "insert"
      (hash "tab" ":hiss-accept"
            "C-space" ":hiss-try")))
