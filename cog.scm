;; Package manifest for forge, in the format steel-pty uses. The dylib entry
;; names the cargo package; forge builds it into libhiss.
(define package-name 'hiss)
(define version "0.1.0")
(define dependencies '())
(define dylibs '((#:name "hiss")))
