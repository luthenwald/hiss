;; Sum the squares of a list of numbers.
(define (sum-of-squares numbers)
   (foldl (lambda (n total) <|>)
          0
          numbers))
