(analyzed-ir (compiler-version "0.33.122")
 (language-version "0.25.107") (runtime-version "0.18.107")
 (exports
   (spend . %spend.107)
   (zerocash_mint . %zerocash_mint.108))
 (contract-types)
 (kernel-declaration (%kernel.153 () (exported #f) (Kernel)))
 (public-ledger-declaration
   (public-ledger-array
     (%nullifiers.137
       (0)
       (exported #f)
       (Set (tstruct nullifier (bytes (tbytes 32)))))
     (%commitments.132
       (1)
       (exported #f)
       (HistoricMerkleTree
         32
         (tstruct commitment (bytes (tbytes 32)))))
     (%ciphertexts.130
       (2)
       (exported #f)
       (__compact_Cell (topaque "Uint8Array"))))
   (constructor () (tuple)))
 (circuit %merkleTreePathRoot.134 (exported #f) (pure #t) (proof #f)
   ((%path.148
      (tstruct
        MerkleTreePath
        (leaf (tstruct commitment (bytes (tbytes 32))))
        (path
          (tvector
            32
            (tstruct
              MerkleTreePathEntry
              (sibling
                (tstruct MerkleTreeDigest (field (tfield (field-native)))))
              (goes_left (tboolean))))))))
   (tstruct MerkleTreeDigest (field (tfield (field-native))))
   (return
     (new (tstruct
            MerkleTreeDigest
            (field (tfield (field-native))))
          (fold
            32
            (fref %merkleTreePathEntryRoot.147)
            ((call
               %degradeToTransient.144
               (call
                 %persistentHash.145
                 (new (tstruct
                        LeafPreimage
                        (domain_sep (tbytes 6))
                        (data (tstruct commitment (bytes (tbytes 32)))))
                      '#vu8(109 100 110 58 108 104)
                      (elt-ref (var-ref %path.148) leaf 0))))
              (tfield (field-native)))
            ((elt-ref (var-ref %path.148) path 1)
              (tvector
                32
                (tstruct
                  MerkleTreePathEntry
                  (sibling
                    (tstruct
                      MerkleTreeDigest
                      (field (tfield (field-native)))))
                  (goes_left (tboolean))))
              (tstruct
                MerkleTreePathEntry
                (sibling
                  (tstruct
                    MerkleTreeDigest
                    (field (tfield (field-native)))))
                (goes_left (tboolean))))))))
 (circuit %merkleTreePathEntryRoot.147 (exported #f) (pure #t)
   (proof #f)
   ((%recursiveDigest.150 (tfield (field-native)))
     (%entry.149
       (tstruct
         MerkleTreePathEntry
         (sibling
           (tstruct MerkleTreeDigest (field (tfield (field-native)))))
         (goes_left (tboolean)))))
   (tfield (field-native))
   (let* (((%left.151 (tfield (field-native))) (if (elt-ref
                                                     (var-ref %entry.149)
                                                     goes_left
                                                     1)
                                                   (var-ref
                                                     %recursiveDigest.150)
                                                   (elt-ref
                                                     (elt-ref
                                                       (var-ref %entry.149)
                                                       sibling
                                                       0)
                                                     field
                                                     0))))
     (let* (((%right.152 (tfield (field-native))) (if (elt-ref
                                                        (var-ref
                                                          %entry.149)
                                                        goes_left
                                                        1)
                                                      (elt-ref
                                                        (elt-ref
                                                          (var-ref
                                                            %entry.149)
                                                          sibling
                                                          0)
                                                        field
                                                        0)
                                                      (var-ref
                                                        %recursiveDigest.150))))
       (return
         (call
           %transientHash.146
           (tuple
             (single (var-ref %left.151))
             (single (var-ref %right.152))))))))
 (native %transientHash.146
   (entry "__compactRuntime.transientHash" circuit)
   (type-arguments (tvector 2 (tfield (field-native))))
   ((%value.154 (tvector 2 (tfield (field-native)))))
   (tfield (field-native)))
 (native %persistentHash.117
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments (tbytes 32)) ((%value.155 (tbytes 32)))
   (tbytes 32))
 (native %persistentHash.110
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments (tvector 4 (tbytes 32)))
   ((%value.156 (tvector 4 (tbytes 32)))) (tbytes 32))
 (native %persistentHash.145
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments
     (tstruct
       LeafPreimage
       (domain_sep (tbytes 6))
       (data (tstruct commitment (bytes (tbytes 32))))))
   ((%value.157
      (tstruct
        LeafPreimage
        (domain_sep (tbytes 6))
        (data (tstruct commitment (bytes (tbytes 32)))))))
   (tbytes 32))
 (native %degradeToTransient.144
   (entry "__compactRuntime.degradeToTransient" circuit)
   (type-arguments) ((%x.158 (tbytes 32)))
   (tfield (field-native)))
 (witness
   %private$zk_secret_key.119
   ()
   (tstruct zk_secret_key (bytes (tbytes 32))))
 (witness
   %private$remove_coin.129
   ((%coin.159
      (tstruct
        coin_info
        (nonce (tstruct Nonce (bytes (tbytes 32))))
        (opening (tstruct opening (bytes (tbytes 32)))))))
   (ttuple))
 (witness
   %private$zk_public_key.139
   ()
   (tstruct zk_public_key (bytes (tbytes 32))))
 (witness
   %private$add_coin.143
   ((%coin.160
      (tstruct
        coin_info
        (nonce (tstruct Nonce (bytes (tbytes 32))))
        (opening (tstruct opening (bytes (tbytes 32)))))))
   (ttuple))
 (witness
   %context$path_of.123
   ((%cm.161 (tstruct commitment (bytes (tbytes 32)))))
   (tstruct
     MerkleTreePath
     (leaf (tstruct commitment (bytes (tbytes 32))))
     (path
       (tvector
         32
         (tstruct
           MerkleTreePathEntry
           (sibling
             (tstruct MerkleTreeDigest (field (tfield (field-native)))))
           (goes_left (tboolean)))))))
 (witness
   %context$new_coin_info.125
   ()
   (tstruct
     coin_info
     (nonce (tstruct Nonce (bytes (tbytes 32))))
     (opening (tstruct opening (bytes (tbytes 32))))))
 (witness
   %context$encrypt.128
   ((%pk.162 (topaque "Uint8Array"))
     (%coin.163
       (tstruct
         coin_info
         (nonce (tstruct Nonce (bytes (tbytes 32))))
         (opening (tstruct opening (bytes (tbytes 32)))))))
   (topaque "Uint8Array"))
 (circuit %spend.107 (exported #t) (pure #f) (proof #t)
   ((%dest_public_key.127
      (tstruct
        public_key
        (zk (tstruct zk_public_key (bytes (tbytes 32))))
        (encryption (topaque "Uint8Array"))))
     (%input_coin.120
       (tstruct
         coin_info
         (nonce (tstruct Nonce (bytes (tbytes 32))))
         (opening (tstruct opening (bytes (tbytes 32)))))))
   (ttuple)
   (seq (let* (((%source_secret_key.121
                  (tstruct zk_secret_key (bytes (tbytes 32)))) (call
                                                                 %private$zk_secret_key.119)))
          (let* (((%old_nullifier.138
                    (tstruct nullifier (bytes (tbytes 32)))) (call
                                                               %derive_nullifier.113
                                                               (var-ref
                                                                 %input_coin.120)
                                                               (var-ref
                                                                 %source_secret_key.121))))
            (seq (assert
                   (if (public-ledger %nullifiers.137 read (0) member (tboolean)
                         (instructions (dup (n 0))
                           (idx (cached #f)
                                (pushPath #f)
                                (path ((align 0 1))))
                           (push
                             (storage #f)
                             (value
                               (state-value
                                 cell
                                 (var-ref %old_nullifier.138))))
                           (member) (popeq (cached #t) (result (void))))
                         (var-ref %old_nullifier.138))
                       '#f
                       '#t)
                   "spend: Coin already spent")
                 (public-ledger %nullifiers.137 update (0) insert (ttuple)
                   (instructions (idx (cached #f) (pushPath #t) (path ((align 0 1))))
                     (push
                       (storage #f)
                       (value
                         (state-value cell (var-ref %old_nullifier.138))))
                     (push (storage #t) (value (state-value null)))
                     (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
                   (var-ref %old_nullifier.138))
                 (let* (((%source_public_key.122
                           (tstruct zk_public_key (bytes (tbytes 32)))) (call
                                                                          %derive_zk_public_key.116
                                                                          (var-ref
                                                                            %source_secret_key.121))))
                   (let* (((%old_commitment.124
                             (tstruct commitment (bytes (tbytes 32)))) (call
                                                                         %commitment_from_coin_info.109
                                                                         (var-ref
                                                                           %input_coin.120)
                                                                         (var-ref
                                                                           %source_public_key.122))))
                     (let* (((%commitment_path.135
                               (tstruct
                                 MerkleTreePath
                                 (leaf
                                   (tstruct
                                     commitment
                                     (bytes (tbytes 32))))
                                 (path
                                   (tvector
                                     32
                                     (tstruct
                                       MerkleTreePathEntry
                                       (sibling
                                         (tstruct
                                           MerkleTreeDigest
                                           (field
                                             (tfield (field-native)))))
                                       (goes_left (tboolean))))))) (call
                                                                     %context$path_of.123
                                                                     (var-ref
                                                                       %old_commitment.124))))
                       (seq (assert
                              (if (let* (((%tmp.136
                                            (tstruct
                                              MerkleTreeDigest
                                              (field
                                                (tfield (field-native))))) (call
                                                                             %merkleTreePathRoot.134
                                                                             (var-ref
                                                                               %commitment_path.135))))
                                    (public-ledger %commitments.132 read (1) checkRoot
                                      (tboolean)
                                      (instructions (dup (n 0))
                                        (idx (cached #f)
                                             (pushPath #f)
                                             (path ((align 1 1))))
                                        (idx (cached #f)
                                             (pushPath #f)
                                             (path ((align 2 1))))
                                        (push
                                          (storage #f)
                                          (value
                                            (state-value
                                              cell
                                              (var-ref %tmp.136))))
                                        (member)
                                        (popeq
                                          (cached #t)
                                          (result (void))))
                                      (var-ref %tmp.136)))
                                  (== (tstruct
                                        commitment
                                        (bytes (tbytes 32)))
                                      (var-ref %old_commitment.124)
                                      (elt-ref
                                        (var-ref %commitment_path.135)
                                        leaf
                                        0))
                                  '#f)
                              "spend: Illegal state: merkle path not recognized by public state")
                            (let* (((%fresh_coin_info.126
                                      (tstruct
                                        coin_info
                                        (nonce
                                          (tstruct
                                            Nonce
                                            (bytes (tbytes 32))))
                                        (opening
                                          (tstruct
                                            opening
                                            (bytes (tbytes 32)))))) (call
                                                                      %context$new_coin_info.125)))
                              (let* (((%fresh_commitment.133
                                        (tstruct
                                          commitment
                                          (bytes (tbytes 32)))) (call
                                                                  %commitment_from_coin_info.109
                                                                  (var-ref
                                                                    %fresh_coin_info.126)
                                                                  (elt-ref
                                                                    (var-ref
                                                                      %dest_public_key.127)
                                                                    zk
                                                                    0))))
                                (seq (public-ledger %commitments.132 update (1) insert
                                       (ttuple)
                                       (instructions
                                         (idx (cached #f)
                                              (pushPath #t)
                                              (path ((align 1 1))))
                                         (idx (cached #f)
                                              (pushPath #t)
                                              (path ((align 0 1))))
                                         (dup (n 2))
                                         (idx (cached #f)
                                              (pushPath #f)
                                              (path ((align 1 1))))
                                         (push
                                           (storage #t)
                                           (value
                                             (state-value
                                               cell
                                               (leaf-hash
                                                 (var-ref
                                                   %fresh_commitment.133)))))
                                         (ins (cached #f) (n 1))
                                         (ins (cached #t) (n 1))
                                         (idx (cached #f)
                                              (pushPath #t)
                                              (path ((align 1 1))))
                                         (addi (immediate 1))
                                         (ins (cached #t) (n 1))
                                         (idx (cached #f)
                                              (pushPath #t)
                                              (path ((align 2 1))))
                                         (dup (n 2))
                                         (idx (cached #f)
                                              (pushPath #f)
                                              (path ((align 0 1))))
                                         (root)
                                         (push
                                           (storage #t)
                                           (value (state-value null)))
                                         (ins (cached #f) (n 1))
                                         (ins (cached #t) (n 2)))
                                       (var-ref %fresh_commitment.133))
                                     (let* (((%ciphertext.131
                                               (topaque "Uint8Array")) (call
                                                                         %context$encrypt.128
                                                                         (elt-ref
                                                                           (var-ref
                                                                             %dest_public_key.127)
                                                                           encryption
                                                                           1)
                                                                         (var-ref
                                                                           %fresh_coin_info.126))))
                                       (seq (public-ledger %ciphertexts.130 write (2)
                                              write (ttuple)
                                              (instructions
                                                (push
                                                  (storage #f)
                                                  (value
                                                    (state-value
                                                      cell
                                                      (align 2 1))))
                                                (push
                                                  (storage #t)
                                                  (value
                                                    (state-value
                                                      cell
                                                      (var-ref
                                                        %ciphertext.131))))
                                                (ins (cached #f) (n 1)))
                                              (var-ref %ciphertext.131))
                                            (call
                                              %private$remove_coin.129
                                              (var-ref
                                                %input_coin.120))))))))))))))
        (return (tuple))))
 (circuit %zerocash_mint.108 (exported #t) (pure #f) (proof #t) ()
   (ttuple)
   (seq (let* (((%coin.140
                  (tstruct
                    coin_info
                    (nonce (tstruct Nonce (bytes (tbytes 32))))
                    (opening (tstruct opening (bytes (tbytes 32)))))) (call
                                                                        %context$new_coin_info.125)))
          (let* (((%pk.141
                    (tstruct zk_public_key (bytes (tbytes 32)))) (call
                                                                   %private$zk_public_key.139)))
            (seq (call %private$add_coin.143 (var-ref %coin.140))
                 (let* (((%cm.142 (tstruct commitment (bytes (tbytes 32)))) (call
                                                                              %commitment_from_coin_info.109
                                                                              (var-ref
                                                                                %coin.140)
                                                                              (var-ref
                                                                                %pk.141))))
                   (public-ledger %commitments.132 update (1) insert (ttuple)
                     (instructions (idx (cached #f) (pushPath #t) (path ((align 1 1))))
                       (idx (cached #f) (pushPath #t) (path ((align 0 1))))
                       (dup (n 2))
                       (idx (cached #f) (pushPath #f) (path ((align 1 1))))
                       (push
                         (storage #t)
                         (value
                           (state-value
                             cell
                             (leaf-hash (var-ref %cm.142)))))
                       (ins (cached #f) (n 1)) (ins (cached #t) (n 1))
                       (idx (cached #f) (pushPath #t) (path ((align 1 1))))
                       (addi (immediate 1)) (ins (cached #t) (n 1))
                       (idx (cached #f) (pushPath #t) (path ((align 2 1))))
                       (dup (n 2))
                       (idx (cached #f) (pushPath #f) (path ((align 0 1))))
                       (root)
                       (push (storage #t) (value (state-value null)))
                       (ins (cached #f) (n 1)) (ins (cached #t) (n 2)))
                     (var-ref %cm.142))))))
        (return (tuple))))
 (circuit %derive_nullifier.113 (exported #f) (pure #t) (proof #f)
   ((%coin.114
      (tstruct
        coin_info
        (nonce (tstruct Nonce (bytes (tbytes 32))))
        (opening (tstruct opening (bytes (tbytes 32))))))
     (%sk.115 (tstruct zk_secret_key (bytes (tbytes 32)))))
   (tstruct nullifier (bytes (tbytes 32)))
   (return
     (new (tstruct nullifier (bytes (tbytes 32)))
          (call
            %persistentHash.110
            (tuple
              (single
                '#vu8(108 97 114 101 115 58 122 101 114 111 99 97 115 104
                      58 99 111 109 109 105 116 0 0 0 0 0 0 0 0 0 0 0))
              (single
                (elt-ref (elt-ref (var-ref %coin.114) nonce 0) bytes 0))
              (single
                (elt-ref (elt-ref (var-ref %coin.114) opening 1) bytes 0))
              (single (elt-ref (var-ref %sk.115) bytes 0)))))))
 (circuit %derive_zk_public_key.116 (exported #f) (pure #t)
   (proof #f)
   ((%sk.118 (tstruct zk_secret_key (bytes (tbytes 32)))))
   (tstruct zk_public_key (bytes (tbytes 32)))
   (return
     (new (tstruct zk_public_key (bytes (tbytes 32)))
          (call
            %persistentHash.117
            (elt-ref (var-ref %sk.118) bytes 0)))))
 (circuit %commitment_from_coin_info.109 (exported #f) (pure #t)
   (proof #f)
   ((%coin.111
      (tstruct
        coin_info
        (nonce (tstruct Nonce (bytes (tbytes 32))))
        (opening (tstruct opening (bytes (tbytes 32))))))
     (%pk.112 (tstruct zk_public_key (bytes (tbytes 32)))))
   (tstruct commitment (bytes (tbytes 32)))
   (return
     (new (tstruct commitment (bytes (tbytes 32)))
          (call
            %persistentHash.110
            (tuple
              (single
                '#vu8(108 97 114 101 115 58 122 101 114 111 99 97 115 104
                      58 99 111 109 109 105 116 0 0 0 0 0 0 0 0 0 0 0))
              (single
                (elt-ref (elt-ref (var-ref %coin.111) nonce 0) bytes 0))
              (single
                (elt-ref (elt-ref (var-ref %coin.111) opening 1) bytes 0))
              (single (elt-ref (var-ref %pk.112) bytes 0))))))))
