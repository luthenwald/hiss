module Monads where

import Prelude hiding (Functor(..), Applicative(..), Monad(..), Maybe(..))

data Maybe a = Nothing | Just a
   deriving (Show)

class Functor f where
   fmap :: (a -> b) -> f a -> f b

  <|>
