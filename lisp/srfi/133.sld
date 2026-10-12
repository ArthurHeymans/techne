(define-library (srfi 133)
  (import (scheme base))
  (import (scheme cxr))
  ;; Procedures supplied by (scheme base).
  (export make-vector vector vector? vector-length vector-ref vector-set!
          vector-copy vector-copy! vector-append vector-fill!
          vector-map vector-for-each vector->list list->vector
          vector->string string->vector)
  ;; Constructors 
  (export vector-unfold vector-unfold-right vector-reverse-copy 
          vector-concatenate vector-append-subvectors)
  ;; Predicates 
  (export vector-empty? vector=)
  ;; Iteration 
  (export vector-fold vector-fold-right vector-map!
          vector-count vector-cumulate)
  ;; Searching 
  (export vector-index vector-index-right vector-skip vector-skip-right 
          vector-binary-search vector-any vector-every vector-partition)
  ;; Mutators 
  (export vector-swap! vector-reverse! 
          vector-reverse-copy! vector-unfold! vector-unfold-right!)
  ;; Conversion 
  (export reverse-vector->list reverse-list->vector)
  (include "133/vectors-impl.scm")
)
