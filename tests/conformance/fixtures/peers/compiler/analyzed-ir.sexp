(analyzed-ir (compiler-version "0.35.103-dev")
  (language-version "0.27.0") (runtime-version "0.20.101")
  (exports
    (peer . %peer.2)
    (set_small . %set_small.3)
    (small . %small.0)
    (swap_peer . %swap_peer.1))
  (contract-types)
  (kernel-declaration (%kernel.8 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%peer.2
        (0)
        (exported #t)
        (__compact_Cell (tcontract Peer (ping #f () (ttuple)))))
      (%small.0
        (1)
        (exported #t)
        (__compact_Cell (tunsigned 16777215))))
    (constructor () (tuple)))
  (circuit %swap_peer.1 (exported #t) (pure #f) (proof #t)
    ((%p.5 (tcontract Peer (ping #f () (ttuple)))))
    (tcontract Peer (ping #f () (ttuple)))
    (let* (((%old.4 (tcontract Peer (ping #f () (ttuple)))) (public-ledger %peer.2 read
                                                              (0) read
                                                              (tcontract
                                                                Peer
                                                                (ping
                                                                  #f
                                                                  ()
                                                                  (ttuple)))
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
                                                                    (void)))))))
      (seq (public-ledger %peer.2 write (0) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 0 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %p.5))))
               (ins (cached #f) (n 1)))
             (var-ref %p.5))
           (return (var-ref %old.4)))))
  (circuit %set_small.3 (exported #t) (pure #f) (proof #t)
    ((%v.7 (tunsigned 16777215))) (tunsigned 16777215)
    (let* (((%old.6 (tunsigned 16777215)) (public-ledger %small.0 read (1) read
                                            (tunsigned 16777215)
                                            (instructions
                                              (dup (n 0))
                                              (idx (cached #f)
                                                   (pushPath #f)
                                                   (path ((align 1 1))))
                                              (popeq
                                                (cached #f)
                                                (result (void)))))))
      (seq (public-ledger %small.0 write (1) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 1 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %v.7))))
               (ins (cached #f) (n 1)))
             (var-ref %v.7))
           (return (var-ref %old.6))))))
