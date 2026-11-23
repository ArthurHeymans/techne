;;; Seven sample messages, a conversation among them, for the editor and
;;; terminal mail tests: plain and HTML text, a quotation, an attachment,
;;; read, unread and starred. Entirely synthetic.
(import (scheme base) (srfi 1) (techne editor) (techne mail store))
(provide mail-test-seed-samples!)

(define samples
  (list
   (list "Nora Lind <nora@example.org>" "A quieter inbox" "Wed, 4 Nov 2026 09:15:00 +0100"
         "Content-Type: text/plain; charset=UTF-8\n\nHi Arthur,\n\nA small thought: the inbox should feel like a reading room, not a queue of obligations.\n\nNo badges asking for attention. Just the people and conversations you came here for.\n\n— Nora\n" '())
   (list "Builds <builds@example.org>" "Nightly build is ready" "Thu, 5 Nov 2026 07:30:00 +0100"
         "Content-Type: text/plain; charset=UTF-8\n\nAll checks passed.\n\n    cargo test --release\n\nThe new editor build is ready when you are.\n" '("\\Seen"))
   (list "Mika Chen <mika@example.org>" "Re: Notes from the workshop" "Thu, 5 Nov 2026 10:05:00 +0100"
         "Content-Type: text/plain; charset=UTF-8\n\nThanks! I added the two points we discussed.\n\nThe key idea is keeping policy in Scheme and making the underlying mechanisms small.\n\n> On Wednesday, Arthur wrote:\n> Could we make this extensible without adding\n> another configuration language?\n\nYes. That is exactly the point.\n" '())
   (list "Ada Moreno <ada@example.org>" "Coffee next week?" "Thu, 5 Nov 2026 11:45:00 +0100"
         "Content-Type: text/plain; charset=UTF-8\n\nHi!\n\nAre you around on Tuesday? There is a new place by the station.\n\nNo agenda, just coffee.\n\nAda\n" '("\\Seen"))
   (list "Lea Wagner <lea@example.org>" "Sketches for the reading room" "Thu, 5 Nov 2026 13:20:00 +0100"
         "MIME-Version: 1.0\nContent-Type: multipart/mixed; boundary=sketch\n\n--sketch\nContent-Type: text/plain; charset=UTF-8\n\nThe notes are attached. A little more room for reading, a little less chrome.\n\n--sketch\nContent-Type: text/plain; name=reading-notes.txt\nContent-Disposition: attachment; filename=reading-notes.txt\nContent-Transfer-Encoding: base64\n\nUm9vbSB0byByZWFkLgo=\n--sketch--\n" '("\\Flagged"))
   (list "Elias Hart <elias@example.org>" "A place for the longer conversations" "Thu, 5 Nov 2026 14:32:00 +0100"
         "MIME-Version: 1.0\nContent-Type: text/html; charset=UTF-8\n\n<html><body><p>Hi Arthur,</p><p>I have been thinking about what makes email worth keeping: not the volume, but the <strong>conversations we return to</strong>.</p><h2>Make room for the message</h2><p>A good reader should get out of the way. Clear typography, thoughtful spacing, and the original structure — without tracking pixels or a miniature web browser.</p><ul><li>Search when you know what you need.</li><li>Groups when you want to catch up.</li><li>Scheme when you want to make it yours.</li></ul><blockquote><p>Tools should feel like a place you can inhabit, not a service you rent.</p></blockquote><p>I put a few <a href=\"https://example.org/notes\">notes on the reading experience</a> together. Curious what you think.</p><p>Until next time,<br>Elias</p></body></html>" '())
   ;; A reply to Mika's message, drawn on the row under it.
   (list "Jonas Berg <jonas@example.org>" "Re: Notes from the workshop" "Thu, 5 Nov 2026 10:40:00 +0100"
         "In-Reply-To: <sample-2@example.org>\nReferences: <sample-2@example.org>\nContent-Type: text/plain; charset=UTF-8\n\nAgreed. I will write up the list of mechanisms before Friday.\n\nJonas\n" '("\\Seen"))))

(define (mail-test-seed-samples! path)
  "Seed the cache at PATH with the samples, of the account Personal.
The account is configured, sending to an invalid host, and never synced."
  (let ((db (mail-store-open path)))
    (for-each (lambda (sample i)
                (mail-store-import! db "Personal" "INBOX" (number->string i)
                                    (string-append "From: " (car sample) "\nTo: Arthur <arthur@example.org>\nSubject: " (cadr sample)
                                                   "\nDate: " (caddr sample) "\nMessage-ID: <sample-" (number->string i) "@example.org>\n"
                                                   (list-ref sample 3))
                                    (list-ref sample 4)))
              samples (iota (length samples)))
    (mail-store-close db))
  (set-option! 'mail-database path)
  (set-option! 'mail-accounts '(((id . "Personal") (kind . maildir) (path . "/missing-techne-mail-samples")
                                 (from . "Arthur <arthur@example.org>") (smtp-host . "smtp.invalid")))))
