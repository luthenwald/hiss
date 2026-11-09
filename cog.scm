;; Package manifest for forge, in the format steel-pty uses. The dylib entry
;; names the cargo package; forge builds it into libhiss.
(define package-name 'hiss)
(define version "0.0.1")
(define dependencies '())
(define dylibs '((#:name "hiss")))
