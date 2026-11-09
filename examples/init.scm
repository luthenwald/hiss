;; How a user loads hiss, in ~/.config/helix/init.scm.
;; `just hx` runs the fork with this directory as its Steel config.

(require "hiss/hiss.scm")

;; Keys, once :hiss-accept and :hiss-clear exist (roadmap step 04):
;;
;; (add-global-keybinding
;;    (hash "insert"
;;       (hash "tab" ":hiss-accept"
;;             "C-g" ":hiss-clear")))
