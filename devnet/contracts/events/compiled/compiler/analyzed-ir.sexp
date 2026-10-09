(analyzed-ir (compiler-version "0.35.103-dev")
  (language-version "0.27.0") (runtime-version "0.20.101")
  (exports
    (last . %last.35)
    (record . %record.36)
    (round . %round.34))
  (contract-types)
  (kernel-declaration (%kernel.40 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%last.35 (0) (exported #t) (__compact_Cell (tbytes 32)))
      (%round.34 (1) (exported #t) (Counter)))
    (constructor () (tuple)))
  (circuit %record.36 (exported #t) (pure #f) (proof #t)
    ((%name.38 (tbytes 32))) (ttuple)
    (seq (public-ledger %last.35 write (0) write (ttuple)
           (instructions
             (push (storage #f) (value (state-value cell (align 0 1))))
             (push
               (storage #t)
               (value (state-value cell (var-ref %name.38))))
             (ins (cached #f) (n 1)))
           (var-ref %name.38))
         (emit 1 10 288
           (let* (((%t.39
                     (tstruct
                       Misc
                       (name (tbytes 32))
                       (payload (tbytes 256)))) (new (tstruct
                                                       Misc
                                                       (name (tbytes 32))
                                                       (payload
                                                         (tbytes 256)))
                                                     (public-ledger %last.35 read (0)
                                                       read (tbytes 32)
                                                       (instructions
                                                         (dup (n 0))
                                                         (idx (cached #f)
                                                              (pushPath #f)
                                                              (path
                                                                ((align
                                                                   0
                                                                   1))))
                                                         (popeq
                                                           (cached #f)
                                                           (result
                                                             (void)))))
                                                     '#vu8(109 105 100 110
                                                           105 103 104 116
                                                           45 114 115 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0 0 0 0 0 0 0
                                                           0 0))))
             (vector->bytes
               288
               (vector
                 (spread
                   32
                   (bytes->vector 32 (elt-ref (var-ref %t.39) name 0)))
                 (spread
                   256
                   (bytes->vector
                     256
                     (elt-ref (var-ref %t.39) payload 1))))))
           (instructions
             (push
               (storage #f)
               (value
                 (state-value
                   array
                   (state-value cell (align 1 4))
                   (state-value cell (align 10 1))
                   (state-value
                     cell
                     (let* (((%t.39
                               (tstruct
                                 Misc
                                 (name (tbytes 32))
                                 (payload (tbytes 256)))) (new (tstruct
                                                                 Misc
                                                                 (name
                                                                   (tbytes
                                                                     32))
                                                                 (payload
                                                                   (tbytes
                                                                     256)))
                                                               (public-ledger %last.35
                                                                 read (0)
                                                                 read
                                                                 (tbytes
                                                                   32)
                                                                 (instructions
                                                                   (dup (n 0))
                                                                   (idx (cached
                                                                          #f)
                                                                        (pushPath
                                                                          #f)
                                                                        (path
                                                                          ((align
                                                                             0
                                                                             1))))
                                                                   (popeq
                                                                     (cached
                                                                       #f)
                                                                     (result
                                                                       (void)))))
                                                               '#vu8(109
                                                                     105
                                                                     100
                                                                     110
                                                                     105
                                                                     103
                                                                     104
                                                                     116 45
                                                                     114
                                                                     115 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0 0 0
                                                                     0))))
                       (vector->bytes
                         288
                         (vector
                           (spread
                             32
                             (bytes->vector
                               32
                               (elt-ref (var-ref %t.39) name 0)))
                           (spread
                             256
                             (bytes->vector
                               256
                               (elt-ref (var-ref %t.39) payload 1))))))))))
             (log)))
         (let* (((%tmp.37 (tunsigned 65535)) (safe-cast
                                               (tunsigned 65535)
                                               (tunsigned 1)
                                               '1)))
           (public-ledger %round.34 update (1) increment (ttuple)
             (instructions
               (idx (cached #f) (pushPath #t) (path ((align 1 1))))
               (addi (immediate (value->int (var-ref %tmp.37))))
               (ins (cached #t) (n 1)))
             (var-ref %tmp.37)))
         (return (tuple)))))
