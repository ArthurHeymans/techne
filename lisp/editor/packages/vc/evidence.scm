;;; Read the durable observation display format without changing disk records.
(provide vc-evidence-summary vc-evidence-observed-id)

(define (field observation prefix)
  (and observation
       (let ((line (find (lambda (line) (string-prefix? prefix line)) (string-split observation "; "))))
         (and line (substring line (string-length prefix))))))

(define (word text)
  (and text (car (string-split text " "))))

(define (observed-head observation)
  (word (field observation "Git HEAD ")))

(define (observation-oid? text)
  (and text (memv (string-length text) '(40 64))
       (every (lambda (c) (or (char<=? #\0 c #\9) (char<=? #\a c #\f))) (string->list text))))

(define (vc-evidence-observed-id observation)
  "Return the commit ID OBSERVATION saw at the destination or HEAD, or #f."
  (let* ((tip (field observation "observed tip "))
         (id (if tip (word tip) (observed-head observation))))
    (and (observation-oid? id) id)))

(define (vc-evidence-summary before after)
  "Return what differs between the observations BEFORE and AFTER.
Each is a phrase, as \"HEAD commit changed\"."
  (define (compare a b unchanged changed)
    (and a b (if (string=? a b) unchanged changed)))
  (let* ((old-tip (word (field before "observed tip "))) (tip (field after "observed tip "))
         (tip-summary
          (cond ((not tip) #f)
                ((string-prefix? "unavailable" tip) "Destination tip unavailable")
                ((string-suffix? " (matches requested commit)" tip) "Destination tip matches the requested commit")
                ((not old-tip) #f)
                ((string=? old-tip "unavailable") #f)
                (else (compare old-tip (word tip) "Destination tip unchanged" "Destination tip changed")))))
    (filter string?
            (list tip-summary
                  (compare (observed-head before) (observed-head after) "HEAD commit unchanged" "HEAD commit changed")
                  (compare (field before "index entries SHA-256 ") (field after "index entries SHA-256 ")
                           "index entries unchanged" "index entries changed")
                  (compare (field before "jj operation ") (field after "jj operation ")
                           "Jujutsu operation unchanged" "Jujutsu operation changed")))))
