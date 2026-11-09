;; How a user loads hiss, in ~/.config/helix/init.scm.
;; A proposal (spec 07/08): the require path and the settings function
;; are settled in roadmap steps 01 and 06.

(require "hiss/hiss.scm")

(add-global-keybinding
   (hash "insert"
      (hash "tab" ":hiss-accept"
            "C-g" ":hiss-clear")))
