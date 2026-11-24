;;; Conversation membership from References/In-Reply-To, never subject guessing.
(define-library (mail thread)
  (export mail-thread-members mail-thread-groups mail-thread-tree)
  (import (techne) (mail store) (mail date))
  (begin
    (define (index-from text char start)
      (let loop ((at start))
        (cond ((>= at (string-length text)) #f) ((char=? (string-ref text at) char) at) (else (loop (+ at 1))))))
    (define (ids text)
      (let loop ((at 0) (out '()))
        (let ((left (index-from text #\< at)))
          (if (not left) (reverse out)
              (let ((right (index-from text #\> (+ left 1))))
                (if (not right) (reverse out)
                    (loop (+ right 1) (cons (substring text left (+ right 1)) out))))))))
    (define (message-ids m)
      (delete-duplicates (append (ids (mail-ref m 'message-id ""))
                                 (ids (mail-ref m 'reply-to "")) (ids (mail-ref m 'refs "")))))
    (define (mail-thread-groups messages)
      "Return summaries MESSAGES as conversations, each in MESSAGES' order.
Conversations come in the order of their first members. Membership is as in
`mail-thread-members`: shared Message-ID/References/In-Reply-To within one
account, never subjects. A message naming no ID is a conversation of its own."
      (let ((owner (make-hash-table)) (parent (make-hash-table)) (count 0))
        (define (root g)
          (let ((p (hash-table-ref/default parent g g)))
            (if (= p g) g (let ((r (root p))) (hash-table-set! parent g r) r))))
        (let assign ((rest messages) (assigned '()))
          (if (pair? rest)
              (let* ((m (car rest))
                     (keys (map (lambda (id) (string-append (mail-ref m 'account "") "\x0;" id)) (message-ids m)))
                     (found (delete-duplicates
                             (filter-map (lambda (key) (let ((g (hash-table-ref/default owner key #f))) (and g (root g)))) keys)))
                     ;; The earliest conversation absorbs the others this message joins.
                     (g (if (pair? found) (apply min found) (begin (set! count (+ count 1)) count))))
                (for-each (lambda (other) (hash-table-set! parent other g)) found)
                (for-each (lambda (key) (hash-table-set! owner key g)) keys)
                (assign (cdr rest) (cons (cons g m) assigned)))
              (let ((groups (make-hash-table)))
                ;; ASSIGNED is newest-assigned first: consing rebuilds MESSAGES' order.
                (for-each (lambda (entry)
                            (let ((r (root (car entry))))
                              (hash-table-set! groups r (cons (cdr entry) (hash-table-ref/default groups r '())))))
                          assigned)
                (let order ((rest (reverse assigned)) (seen '()) (out '()))
                  (if (null? rest) (reverse out)
                      (let ((r (root (car (car rest)))))
                        (if (memv r seen) (order (cdr rest) seen out)
                            (order (cdr rest) (cons r seen) (cons (hash-table-ref/default groups r '()) out)))))))))))

    (define (older? a b)
      (let ((ta (or (mail-date->instant (mail-ref a 'date "")) 0))
            (tb (or (mail-date->instant (mail-ref b 'date "")) 0)))
        (if (= ta tb) (< (mail-ref a 'id) (mail-ref b 'id)) (< ta tb))))
    (define (subject-key text)
      ;; A reply's subject is its parent's, whatever prefixes it gathered.
      (let loop ((s (string-trim (string-downcase text))))
        (let ((prefix (find (lambda (p) (string-prefix? p s)) '("re:" "fwd:" "fw:" "aw:"))))
          (if prefix (loop (string-trim (substring s (string-length prefix) (string-length s)))) s))))

        (define (mail-thread-tree group)
      "Return conversation GROUP as rows, a reply under what it answers.
GROUP is newest first, as a query and `mail-thread-groups` give it: no
date is parsed here. Each row is its message with two more entries:
tree, the text drawing its place (empty for a root, else guides and an
arrow), and tree-subject?, #f when its subject only repeats its
parent's. A parent is the In-Reply-To message, else the nearest of
References that GROUP has; a message whose parent is missing is a root.
Roots and replies go oldest first."
      (if (null? (cdr group))
          (list (append '((tree . "") (tree-subject? . #t)) (car group)))
          ;; Tables by Message-ID and by parent: a long conversation is
          ;; laid out in time linear in its length.
          (let ((named (make-hash-table)) (kids (make-hash-table)) (seen (make-hash-table)) (rows '()))
            (define (key m) (if m (mail-ref m 'id) 0))
            (define (parent m)
              (let loop ((candidates (append (ids (mail-ref m 'reply-to "")) (reverse (ids (mail-ref m 'refs ""))))))
                (and (pair? candidates)
                     (let ((p (hash-table-ref/default named (car candidates) #f)))
                       (if (and p (not (eq? p m))) p (loop (cdr candidates)))))))
            ;; Consed from GROUP, newest first: each list is oldest first.
            (define (children p) (hash-table-ref/default kids (key p) '()))
            (define (walk m guides last? depth above)
              (unless (hash-table-ref/default seen (key m) #f)
                (hash-table-set! seen (key m) #t)
                (set! rows
                      (cons (append (list (cons 'tree (if (= depth 0) "" (string-append guides (if last? "╰─► " "├─► "))))
                                          (cons 'tree-subject?
                                                (or (not above)
                                                    (not (string=? (subject-key (mail-ref above 'subject ""))
                                                                   (subject-key (mail-ref m 'subject "")))))))
                                    m)
                            rows))
                (let loop ((rest (children m)))
                  (when (pair? rest)
                    ;; Past six levels replies stop moving right: the subject needs the room.
                    (walk (car rest) (if (or (= depth 0) (> depth 6)) guides (string-append guides (if last? "    " "│   ")))
                          (null? (cdr rest)) (+ depth 1) m)
                    (loop (cdr rest))))))
            (for-each (lambda (m)
                        (for-each (lambda (id) (unless (hash-table-ref/default named id #f) (hash-table-set! named id m)))
                                  (ids (mail-ref m 'message-id ""))))
                      group)
            (for-each (lambda (m)
                        (let ((k (key (parent m))))
                          (hash-table-set! kids k (cons m (hash-table-ref/default kids k '())))))
                      group)
            (for-each (lambda (m) (walk m "" #t 0 #f)) (children #f))
            ;; Messages that only name each other have no root: none is lost.
            (for-each (lambda (m) (walk m "" #t 0 #f)) (reverse group))
            (reverse rows))))

    (define (mail-thread-members messages selected)
      "Return SELECTED's conversation from summaries MESSAGES.
Members follow parsed Date, then cache ID, oldest first.
Membership follows shared Message-ID/References/In-Reply-To within one account.
Missing parents are supported; identical subjects alone never merge messages."
      (let* ((account (mail-ref selected 'account))
             (candidates (filter (lambda (m) (equal? (mail-ref m 'account) account)) messages))
             (known (message-ids selected)))
        (let loop ((rest candidates) (known known) (members (list selected)))
          (let* ((found (filter (lambda (m) (and (not (find (lambda (old) (equal? (mail-ref old 'id) (mail-ref m 'id))) members))
                                               (any (lambda (id) (member id known)) (message-ids m)))) rest)))
            (if (null? found)
                (sort members older?)
                (loop (remove (lambda (m) (find (lambda (f) (equal? (mail-ref m 'id) (mail-ref f 'id))) found)) rest)
                      (delete-duplicates (append known (append-map message-ids found))) (append members found)))))))))
